//! Filename helpers for the screenshot key (pure / unit-tested). The GPU
//! readback + PNG encode lives in `main` (it needs the render context).

/// Convert a Unix timestamp (seconds, UTC) to `YYYY-MM-DD_HHMMSS`.
pub fn timestamp_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let sod = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    let (h, mi, s) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    format!("{y:04}-{m:02}-{d:02}_{h:02}{mi:02}{s:02}")
}

/// Civil date (year, month, day) from days since the Unix epoch — Howard
/// Hinnant's `civil_from_days` (proleptic Gregorian, UTC).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (y + i64::from(m <= 2), m, d)
}

/// A filesystem-safe, lowercase fragment of a preset name (alnum kept, runs of
/// other chars collapsed to a single `-`, trimmed, capped at 40 chars).
pub fn sanitize(name: &str) -> String {
    // Drop a trailing `.milk` so the fragment isn't `...-milk`.
    let name = name.strip_suffix(".milk").unwrap_or(name);
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed: String = out.trim_matches('-').chars().take(40).collect();
    let trimmed = trimmed.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "preset".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(timestamp_utc(0), "1970-01-01_000000");
        // 2021-01-01T00:00:00Z
        assert_eq!(timestamp_utc(1_609_459_200), "2021-01-01_000000");
        // 2009-02-13T23:31:30Z (1234567890)
        assert_eq!(timestamp_utc(1_234_567_890), "2009-02-13_233130");
    }

    #[test]
    fn sanitize_names() {
        assert_eq!(sanitize("martin - lightning [tweak].milk"), "martin-lightning-tweak");
        assert_eq!(sanitize("built-in"), "built-in");
        assert_eq!(sanitize("!!!.milk"), "preset");
        assert_eq!(sanitize("$$$ Royal (186).milk"), "royal-186");
    }
}
