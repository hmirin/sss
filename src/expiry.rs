use anyhow::{bail, Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs() as i64
}

pub fn duration(value: &str) -> Result<Option<i64>> {
    if value == "none" {
        return Ok(None);
    }
    if !value.is_ascii() || value.is_empty() {
        bail!("invalid duration");
    }
    let (digits, unit) = value.split_at(value.len() - 1);
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        "w" => 604800,
        _ => bail!("expected a duration such as 7d, 12h, 30m, or none"),
    };
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        bail!("invalid duration");
    }
    let seconds = digits
        .parse::<i64>()?
        .checked_mul(multiplier)
        .context("duration too large")?;
    if seconds <= 0 {
        bail!("duration must be positive; use none for no expiration");
    }
    now().checked_add(seconds).context("expiration too large")?;
    Ok(Some(seconds))
}

pub fn deadline(seconds: Option<i64>) -> Result<Option<i64>> {
    seconds
        .map(|n| now().checked_add(n).context("expiration too large"))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_durations() {
        assert_eq!(duration("7d").unwrap(), Some(604800));
        assert_eq!(duration("none").unwrap(), None);
        for v in ["", "0s", "-1d", "1.5h", "1", "1日", "9223372036854775807w"] {
            assert!(duration(v).is_err(), "{v}");
        }
    }
}
