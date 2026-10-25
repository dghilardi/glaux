//! Optional unattended Microsoft sign-in with a passkey held by an external authenticator.
//! Glaux never handles passwords, codes or keys: it only enters the configured account,
//! selects its tile, accepts "Stay signed in?" and waits while the authenticator signs.
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub const LOGIN_HOSTS: &[&str] = &["login.microsoftonline.com", "login.microsoft.com"];
const MAX_ACTIONS: u32 = 6;
const ACTION_COOLDOWN: Duration = Duration::from_secs(3);

pub fn validate_account(raw: &str) -> Result<String> {
    let account = raw.trim().to_ascii_lowercase();
    let valid = (3..=254).contains(&account.len())
        && account.bytes().all(|b| b.is_ascii_graphic())
        && account.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.contains('.') && !domain.contains('@')
        });
    ensure!(valid, "Passkey account must be a work email address");
    Ok(account)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Elsewhere,
    Waiting,
    Account,
    Picker,
    StaySignedIn,
    Passkey,
}

/// The page script performs at most one whitelisted action per call.
pub fn step_script(account: &str, act: bool, hosts: &[&str]) -> String {
    let args = json!({"account":account,"act":act,"hosts":hosts});
    format!(
        r#"((a) => {{
    if (location.protocol !== 'https:' && location.hostname !== '127.0.0.1') return {{step:'elsewhere'}};
    if (!a.hosts.includes(location.hostname)) return {{step:'elsewhere'}};
    const visible = e => !!e && e.getClientRects().length > 0 && getComputedStyle(e).visibility !== 'hidden';
    const find = s => [...document.querySelectorAll(s)].find(visible);
    const errors = '#usernameError,#passwordError,#idTD_Error,#service_exception_message';
    if (find(errors)) return {{step:'error'}};
    if (find('#KmsiCheckboxField,input[name=DontShowAgain]')) {{
        const yes = find('#idSIButton9');
        if (!yes) return {{step:'waiting'}};
        if (a.act) yes.click();
        return {{step:'stay_signed_in', acted:a.act}};
    }}
    const user = find('input[name=loginfmt]');
    if (user && !find('#displayName')) {{
        const next = find('#idSIButton9');
        if (!next) return {{step:'waiting'}};
        if (a.act) {{
            Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(user, a.account);
            user.dispatchEvent(new Event('input', {{bubbles:true}}));
            user.dispatchEvent(new Event('change', {{bubbles:true}}));
            next.click();
        }}
        return {{step:'account', acted:a.act}};
    }}
    const tile = [...document.querySelectorAll('[data-test-id]')]
        .find(e => visible(e) && e.getAttribute('data-test-id').toLowerCase() === a.account);
    if (tile) {{
        if (a.act) tile.click();
        return {{step:'picker', acted:a.act}};
    }}
    if (find('input[type=password]')) return {{step:'password'}};
    if (location.pathname.includes('/fido')) return {{step:'passkey'}};
    return {{step:'waiting'}};
}})({args})"#
    )
}

/// Bounds automation: a cooldown between actions and a small total action budget.
pub struct Driver {
    account: String,
    actions: u32,
    last_action: Option<Instant>,
}

impl Driver {
    pub fn new(account: &str) -> Self {
        Self {
            account: account.to_owned(),
            actions: 0,
            last_action: None,
        }
    }

    pub fn script(&self, now: Instant, hosts: &[&str]) -> String {
        let act = self.actions < MAX_ACTIONS
            && self
                .last_action
                .is_none_or(|last| now.duration_since(last) >= ACTION_COOLDOWN);
        step_script(&self.account, act, hosts)
    }

    pub fn record(&mut self, value: &Value, now: Instant) -> Result<Step> {
        let step = match value["step"].as_str() {
            Some("elsewhere") => Step::Elsewhere,
            Some("account") => Step::Account,
            Some("picker") => Step::Picker,
            Some("stay_signed_in") => Step::StaySignedIn,
            Some("passkey") => Step::Passkey,
            Some("password") => bail!(
                "Microsoft asked for a password; passkey sign-in needs attention, run `glaux login`"
            ),
            Some("error") => {
                bail!("Microsoft sign-in reported an error; run `glaux login`")
            }
            _ => Step::Waiting,
        };
        if value["acted"] == true {
            self.actions += 1;
            self.last_action = Some(now);
        } else if self.actions >= MAX_ACTIONS
            && matches!(step, Step::Account | Step::Picker | Step::StaySignedIn)
        {
            bail!("Passkey sign-in did not progress; run `glaux login`");
        }
        Ok(step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_plain_work_addresses() {
        assert_eq!(
            validate_account(" User@Example.TEST ").unwrap(),
            "user@example.test"
        );
        for raw in [
            "",
            "user",
            "@example.test",
            "user@localhost",
            "a@b@example.test",
            "user name@example.test",
            "user@exa\u{202e}mple.test",
            "user@exa\tmple.test",
        ] {
            assert!(validate_account(raw).is_err(), "{raw:?}");
        }
    }

    #[test]
    fn script_embeds_account_as_data() {
        let script = step_script("x'});alert(1)//@example.test", true, LOGIN_HOSTS);
        assert!(script.contains(r#""account":"x'});alert(1)//@example.test""#));
    }

    #[test]
    fn stops_on_password_and_errors() {
        let mut driver = Driver::new("user@example.test");
        let now = Instant::now();
        assert!(driver.record(&json!({"step":"password"}), now).is_err());
        assert!(driver.record(&json!({"step":"error"}), now).is_err());
        assert_eq!(
            driver.record(&json!({"step":"passkey"}), now).unwrap(),
            Step::Passkey
        );
    }

    #[test]
    fn rate_limits_and_bounds_actions() {
        let mut driver = Driver::new("user@example.test");
        let start = Instant::now();
        assert!(driver.script(start, LOGIN_HOSTS).contains(r#""act":true"#));
        driver
            .record(&json!({"step":"account","acted":true}), start)
            .unwrap();
        assert!(driver.script(start, LOGIN_HOSTS).contains(r#""act":false"#));
        let mut now = start;
        for _ in 1..MAX_ACTIONS {
            now += ACTION_COOLDOWN;
            assert!(driver.script(now, LOGIN_HOSTS).contains(r#""act":true"#));
            driver
                .record(&json!({"step":"account","acted":true}), now)
                .unwrap();
        }
        now += ACTION_COOLDOWN;
        assert!(driver.script(now, LOGIN_HOSTS).contains(r#""act":false"#));
        assert!(
            driver
                .record(&json!({"step":"account","acted":false}), now)
                .is_err()
        );
    }
}
