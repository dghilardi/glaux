#!/usr/bin/env python3
"""Opt-in live CLI/MCP reads; keep mailbox contents and IDs in memory only."""
import argparse
import asyncio
import json
import os
import signal
import sys

CALL_TIMEOUT = 180
STOP_TIMEOUT = 15
PROTOCOL_VERSION = "2025-11-25"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


async def stop(process):
    if process.returncode is not None:
        return
    # Let Glaux close Chromium and flush the dedicated profile before escalating.
    try:
        if os.name == "nt":
            process.terminate()
        else:
            process.send_signal(signal.SIGINT)
    except ProcessLookupError:
        pass
    try:
        await asyncio.wait_for(process.communicate(), STOP_TIMEOUT)
    except asyncio.TimeoutError:
        if process.returncode is None:
            process.kill()
        await process.communicate()


async def cli(command, *args):
    process = await asyncio.create_subprocess_exec(
        *command, *args, stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.DEVNULL,
    )
    try:
        stdout, _ = await asyncio.wait_for(process.communicate(), CALL_TIMEOUT)
        require(process.returncode == 0, "CLI read failed; run glaux status separately")
        result = json.loads(stdout)
        require(result.get("ok") is True, "CLI did not report success")
        return result["data"]
    finally:
        await stop(process)


async def validate(command):
    page = await cli(command, "list", "--limit", "5")
    require(page.get("messages"), "Inbox is empty; select a nonempty mailbox for this check")
    message_id = page["messages"][0]["id"]
    mail = await cli(command, "read", message_id)
    require(mail.get("id") == message_id, "CLI returned a different message")
    require(isinstance(mail.get("body"), str), "CLI returned no message body")
    require(mail.get("body_type") == "Text", "CLI did not return plain text")
    status = await cli(command, "status")
    require(status.get("mailbox_readable") is True, "Session reuse failed")
    print("CLI: list, plain-text read and session reuse in separate processes passed")

    process = await asyncio.create_subprocess_exec(
        *command, "mcp", stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL,
        limit=32 * 1024 * 1024,
    )

    async def send(message):
        process.stdin.write((json.dumps(message) + "\n").encode())
        await process.stdin.drain()

    async def request(number, method, params=None):
        await send({"jsonrpc": "2.0", "id": number, "method": method,
                    "params": params or {}})
        async with asyncio.timeout(CALL_TIMEOUT):
            while True:
                line = await process.stdout.readline()
                require(line, "MCP closed before returning a response")
                response = json.loads(line)
                if "method" in response:
                    if response.get("method") == "ping" and "id" in response:
                        await send({"jsonrpc": "2.0", "id": response["id"], "result": {}})
                    elif "id" in response:
                        await send({"jsonrpc": "2.0", "id": response["id"],
                                    "error": {"code": -32601, "message": "Method not found"}})
                    continue
                if response.get("id") == number:
                    require("result" in response, "MCP returned a protocol error")
                    return response["result"]

    async def tool(number, name, arguments):
        result = await request(number, "tools/call", {"name": name, "arguments": arguments})
        require(not result.get("isError"), "MCP tool failed; run glaux status separately")
        data = json.loads(result["content"][0]["text"])
        require(data.get("ok") is True, "MCP tool did not report success")
        return data["data"]

    try:
        initialized = await request(1, "initialize", {
            "protocolVersion": PROTOCOL_VERSION, "capabilities": {},
            "clientInfo": {"name": "glaux-live-validation", "version": "1"},
        })
        require(initialized.get("protocolVersion") == PROTOCOL_VERSION,
                "MCP selected an unsupported protocol version")
        await send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        tools = (await request(2, "tools/list"))["tools"]
        require({t["name"] for t in tools} == {"list_messages", "read_message"},
                "Unexpected MCP discovery result")
        require(all(t.get("annotations", {}).get("readOnlyHint") for t in tools),
                "Missing read-only annotations")
        mcp_page = await tool(3, "list_messages", {"limit": 5})
        require(mcp_page.get("messages"), "MCP list returned no messages")
        mcp_mail = await tool(4, "read_message", {"id": message_id})
        require(mcp_mail == mail, "CLI and MCP returned different data for the selected message")
        print("MCP: discovery, list_messages and read_message passed; selected message matches CLI")
    finally:
        process.stdin.close()
        await stop(process)


def main():
    if sys.version_info < (3, 11):
        print("Live validation requires Python 3.11 or newer.")
        raise SystemExit(1)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--glaux", default="glaux", help="Path to the installed Glaux binary")
    parser.add_argument("--data-dir", help="Optional dedicated Glaux data directory")
    args = parser.parse_args()
    command = [args.glaux, "--json"]
    if args.data_dir:
        command.extend(["--data-dir", args.data_dir])
    try:
        asyncio.run(validate(command))
    except (Exception, KeyboardInterrupt):
        # Exceptions can contain protocol/mail data: never print their raw values.
        print("Live validation failed or interrupted. No mailbox contents were printed. "
              "Check glaux status, then retry.")
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
