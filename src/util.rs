//! Small shared helpers: SQL hashing, byte formatting, name derivation, wall-clock
//! timestamps. (Domain vocabulary like `Kind` lives in `crate::model`.)

use std::collections::BTreeSet;
use std::path::Path;

/// Wall-clock `HH:MM:SS` (UTC) for log timestamps — avoids a chrono dependency.
pub fn now_hms() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        % 86_400;
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}

/// A stable FNV-1a hash of the **trimmed** SQL — the tab dirty-tracking baseline.
/// Cheaper than storing/comparing whole strings, and deterministic across runs so
/// a persisted baseline still matches after reload.
pub fn sql_hash(sql: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in sql.trim().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Human-readable byte size (e.g. `1.4 MB`).
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut f = n as f64;
    let mut i = 0;
    while f >= 1024.0 && i < UNITS.len() - 1 {
        f /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{f:.1} {}", UNITS[i])
    }
}

/// Turn a file/dir name into a valid, unique lower_snake SQL identifier.
pub fn derive_table_name(path: &Path, existing: &BTreeSet<String>) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("table");
    let mut base: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if base.is_empty() {
        base = "table".into();
    }
    if base.chars().next().map_or(true, |c| c.is_ascii_digit()) {
        base = format!("t_{base}");
    }
    let mut name = base.clone();
    let mut i = 2;
    while existing.contains(&name) {
        name = format!("{base}_{i}");
        i += 1;
    }
    name
}

/// Split `"1.5G"` → `("1.5", "G")` — the leading numeric run and the trailing unit.
fn split_num_unit(s: &str) -> (&str, &str) {
    let idx = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    (s[..idx].trim(), s[idx..].trim())
}

/// A number with an optional byte-size unit (K/M/G/T, optionally `i`/`B`).
pub fn is_byte_size(v: &str) -> bool {
    let (num, unit) = split_num_unit(v);
    if num.parse::<f64>().is_err() {
        return false;
    }
    unit.is_empty()
        || matches!(
            unit.chars().next().map(|c| c.to_ascii_lowercase()),
            Some('k') | Some('m') | Some('g') | Some('t') | Some('b')
        )
}

/// A number with an optional duration unit (s/m/h).
pub fn is_duration(v: &str) -> bool {
    let (num, unit) = split_num_unit(v);
    if num.parse::<f64>().is_err() {
        return false;
    }
    unit.is_empty()
        || matches!(
            unit.chars().next().map(|c| c.to_ascii_lowercase()),
            Some('s') | Some('m') | Some('h')
        )
}

/// A `±HH:MM` offset (hours 00-14, minutes 00-59) or a named zone (letters, digits, `/_+-`).
pub fn is_time_zone(v: &str) -> bool {
    if let Some(rest) = v.strip_prefix(['+', '-']) {
        let b = rest.as_bytes();
        return rest.len() == 5
            && b[2] == b':'
            && matches!(
                (rest[0..2].parse::<u32>(), rest[3..5].parse::<u32>()),
                (Ok(h), Ok(m)) if h <= 14 && m <= 59
            );
    }
    v.chars().any(|c| c.is_ascii_alphabetic())
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '+' | '-'))
}

pub fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}