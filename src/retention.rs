//! Opt-in local retention for archive categories. Only local EML copies expire;
//! the server mailbox is never modified.
use anyhow::{Result, ensure};
use serde::Deserialize;

/// A retention period written as `<days>d`, for example `"10d"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Retention {
    pub days: u32,
}

impl TryFrom<String> for Retention {
    type Error = anyhow::Error;
    fn try_from(raw: String) -> Result<Self> {
        let days = raw
            .strip_suffix('d')
            .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|n| (1..=36500).contains(n));
        let days = days.ok_or_else(|| anyhow::anyhow!("Retention must look like \"10d\""))?;
        Ok(Self { days })
    }
}

impl Retention {
    pub fn cutoff(&self, now_unix: i64) -> i64 {
        now_unix - i64::from(self.days) * 86_400
    }
}

/// OWA `DateTimeReceived`, e.g. `2026-10-25T10:00:00Z` or with an offset/fraction.
pub fn parse_received(raw: &str) -> Option<i64> {
    let b = raw.as_bytes();
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = raw.get(range)?;
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &raw[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        rest = &fraction[digits..];
    }
    let offset = match rest {
        "Z" | "" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            if rest.len() != 6 || rest.as_bytes()[3] != b':' {
                return None;
            }
            let hours: i64 = rest[1..3].parse().ok()?;
            let minutes: i64 = rest[4..6].parse().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
    };
    // Days from civil date (proleptic Gregorian), see H. Hinnant's algorithms.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

pub fn validate(category: &str) -> Result<()> {
    ensure!(
        !category.is_empty()
            && category.len() <= 64
            && category
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'),
        "Invalid retention category"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_days_only() {
        assert_eq!(Retention::try_from("10d".to_string()).unwrap().days, 10);
        for raw in [
            "",
            "d",
            "10",
            "10h",
            "-1d",
            "0d",
            "1.5d",
            " 10d",
            "99999999999d",
        ] {
            assert!(Retention::try_from(raw.to_string()).is_err(), "{raw:?}");
        }
        assert_eq!(Retention { days: 2 }.cutoff(200_000), 200_000 - 172_800);
    }

    #[test]
    fn parses_owa_received_times() {
        assert_eq!(parse_received("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_received("2026-10-25T10:00:00Z"), Some(1_792_922_400));
        assert_eq!(
            parse_received("2026-10-25T12:00:00+02:00"),
            Some(1_792_922_400)
        );
        assert_eq!(
            parse_received("2026-10-25T10:00:00.1234567Z"),
            Some(1_792_922_400)
        );
        assert_eq!(parse_received("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        for raw in [
            "",
            "2026-10-25",
            "2026-13-01T00:00:00Z",
            "2026-10-25T10:00:00X",
            "2026-10-25T10:00:00.Z",
            "2026-10-25T10:00:00+0200",
            "２026-10-25T10:00:00Z",
        ] {
            assert_eq!(parse_received(raw), None, "{raw:?}");
        }
    }
}
