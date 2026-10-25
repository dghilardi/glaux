"""Synthetic subprocess regressions; never launch Chromium or access a mailbox."""
import asyncio
import contextlib
import importlib.util
import io
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "validate_live", Path(__file__).resolve().parents[1] / "scripts/validate_live.py"
)
live = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(live)

FAKE = r'''
import json
from pathlib import Path
import signal
import sys
import time

mode, directory = sys.argv[1:3]
directory = Path(directory)
def hang():
    def cleanup(*_):
        time.sleep(0.2)
        (directory / "cleaned").touch()
        raise SystemExit(0)
    signal.signal(signal.SIGINT, signal.SIG_IGN if mode == "ignore" else cleanup)
    (directory / "ready").touch()
    while True:
        time.sleep(0.05)

if mode in ("hang", "ignore"):
    hang()

mail = {"id": "synthetic-id", "body": "Dàti sintetici", "body_type": "Text"}
page = {"messages": [mail]}
command = sys.argv[3]
if command != "mcp":
    data = {"list": page, "read": mail, "status": {"mailbox_readable": True}}[command]
    print(json.dumps({"ok": True, "data": data}))
    raise SystemExit(0)

def send(value):
    print(json.dumps(value), flush=True)

for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        version = "1999-01-01" if mode == "wrong-version" else "2025-11-25"
        result = {"protocolVersion": version}
    elif method == "tools/list":
        # Server and client request IDs belong to independent namespaces.
        send({"jsonrpc": "2.0", "id": request["id"], "method": "ping"})
        reply = json.loads(sys.stdin.readline())
        assert reply["id"] == request["id"] and reply["result"] == {}
        result = {"tools": [{"name": name, "annotations": {"readOnlyHint": True}}
                            for name in ("list_messages", "read_message")]}
    elif method == "tools/call":
        if mode == "mcp-hang":
            hang()
        data = page if request["params"]["name"] == "list_messages" else mail
        result = {"content": [{"type": "text", "text": json.dumps({"ok": True, "data": data})}]}
    else:
        continue
    send({"jsonrpc": "2.0", "id": request["id"], "result": result})
'''


class LiveValidationTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name)
        self.fake = self.path / "fake_glaux.py"
        self.fake.write_text(FAKE)

    def command(self, mode):
        return [sys.executable, str(self.fake), mode, str(self.path)]

    async def ready(self):
        async with asyncio.timeout(5):
            while not (self.path / "ready").exists():
                await asyncio.sleep(0.01)

    async def test_mcp_ping_with_same_id_and_private_success_output(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            await live.validate(self.command("normal"))
        self.assertIn("selected message matches CLI", output.getvalue())
        self.assertNotIn("synthetic-id", output.getvalue())
        self.assertNotIn("sintetici", output.getvalue())

    async def test_rejects_unsupported_protocol(self):
        with contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "unsupported protocol"):
                await live.validate(self.command("wrong-version"))

    @unittest.skipIf(os.name == "nt", "POSIX SIGINT cleanup behavior")
    async def test_cancel_allows_cli_cleanup(self):
        operation = asyncio.create_task(live.cli(self.command("hang"), "list"))
        try:
            await self.ready()
        finally:
            operation.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await operation
        self.assertTrue((self.path / "cleaned").exists())

    @unittest.skipIf(os.name == "nt", "POSIX SIGINT cleanup behavior")
    async def test_cancel_during_spawn_retains_child_for_cleanup(self):
        create = asyncio.create_subprocess_exec
        async def delayed_handle(*args, **kwargs):
            process = await create(*args, **kwargs)
            await self.ready()
            await asyncio.sleep(0.1)
            return process
        with patch.object(asyncio, "create_subprocess_exec", delayed_handle):
            operation = asyncio.create_task(live.cli(self.command("hang"), "list"))
            await self.ready()
            operation.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await operation
        self.assertTrue((self.path / "cleaned").exists())

    @unittest.skipIf(os.name == "nt", "POSIX SIGINT cleanup behavior")
    async def test_timeout_allows_cli_cleanup(self):
        with patch.object(live, "CALL_TIMEOUT", 1):
            with self.assertRaises(asyncio.TimeoutError):
                await live.cli(self.command("hang"), "list")
        self.assertTrue((self.path / "cleaned").exists())

    @unittest.skipIf(os.name == "nt", "POSIX SIGINT cleanup behavior")
    async def test_cancel_allows_mcp_cleanup(self):
        with contextlib.redirect_stdout(io.StringIO()):
            operation = asyncio.create_task(live.validate(self.command("mcp-hang")))
            try:
                await self.ready()
            finally:
                operation.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await operation
        self.assertTrue((self.path / "cleaned").exists())

    @unittest.skipIf(os.name == "nt", "POSIX signal escalation behavior")
    async def test_unresponsive_process_is_killed_and_reaped(self):
        process = await asyncio.create_subprocess_exec(*self.command("ignore"))
        try:
            await self.ready()
            with patch.object(live, "STOP_TIMEOUT", 0.1):
                await live.stop(process)
            self.assertIsNotNone(process.returncode)
        finally:
            if process.returncode is None:
                process.kill()
                await process.wait()


if __name__ == "__main__":
    unittest.main()
