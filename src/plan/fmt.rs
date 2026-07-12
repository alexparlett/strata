//! Unit-aware formatters shared by the engine (building metric labels + the
//! self-time chip) and the view, so both render numbers identically.

/// Format a millisecond value for the per-node time chip.
pub fn fmt_ms(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0} ms")
    } else if v >= 1.0 {
        format!("{v:.1} ms")
    } else if v > 0.0 {
        format!("{v:.3} ms")
    } else {
        "0 ms".to_string()
    }
}

/// Format a nanosecond metric value with a human unit (`0`, `842 ns`, `13.9 µs`,
/// `15.6 ms`, `1.20 s`) for the tier-3 grid.
pub fn fmt_ns(ns: u64) -> String {
    let n = ns as f64;
    if ns == 0 {
        "0".to_string()
    } else if n < 1_000.0 {
        format!("{ns} ns")
    } else if n < 1_000_000.0 {
        format!("{:.1} µs", n / 1_000.0)
    } else if n < 1_000_000_000.0 {
        format!("{:.1} ms", n / 1_000_000.0)
    } else {
        format!("{:.2} s", n / 1_000_000_000.0)
    }
}

/// Format a byte count (bytes or memory) with a binary unit (`605 B`, `3.4 KB`,
/// `3.1 MB`).
pub fn fmt_bytes(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if bytes == 0 {
        "0 B".to_string()
    } else if b < K {
        format!("{bytes} B")
    } else if b < K * K {
        format!("{:.1} KB", b / K)
    } else if b < K * K * K {
        format!("{:.1} MB", b / (K * K))
    } else {
        format!("{:.1} GB", b / (K * K * K))
    }
}

/// Group a non-negative integer with thousands separators (`48213` → `48,213`).
pub fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_ms() {
        assert_eq!(fmt_ms(0.0), "0 ms");
        assert_eq!(fmt_ms(0.0006), "0.001 ms");
        assert_eq!(fmt_ms(2.14), "2.1 ms");
        assert_eq!(fmt_ms(842.0), "842 ms");
    }

    #[test]
    fn formats_units() {
        assert_eq!(fmt_ns(0), "0");
        assert_eq!(fmt_ns(842), "842 ns");
        assert_eq!(fmt_ns(15_594_334), "15.6 ms");
        assert_eq!(fmt_bytes(605), "605 B");
        assert_eq!(fmt_bytes(3481), "3.4 KB");
        assert_eq!(fmt_int(48213), "48,213");
    }
}
