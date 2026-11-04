//! Number, byte, duration and width-aware string formatting.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Formats `v` with 3 significant digits and at most 2 decimals, trimming
/// trailing zeros. Returns the digits and whether rounding reached 1000.
fn sig3(v: f64) -> (String, bool) {
    let a = v.abs();
    let decimals = if a >= 100.0 {
        0
    } else if a >= 10.0 {
        1
    } else {
        2
    };
    let mut s = format!("{:.*}", decimals, v);
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if s == "-0" {
        s = "0".into();
    }
    let rounded: f64 = s.parse().unwrap_or(v);
    (s, rounded.abs() >= 1000.0)
}

/// A big-number string and its unit suffix: "", "k", "M" or "G".
pub fn fmt_big(v: f64) -> (String, &'static str) {
    if !v.is_finite() {
        return ("—".into(), "");
    }
    const UNITS: [(f64, &str); 4] = [(1.0, ""), (1e3, "k"), (1e6, "M"), (1e9, "G")];
    let mut idx = UNITS
        .iter()
        .rposition(|(scale, _)| v.abs() >= *scale)
        .unwrap_or(0);
    loop {
        let (s, overflow) = sig3(v / UNITS[idx].0);
        if overflow && idx + 1 < UNITS.len() {
            idx += 1;
            continue;
        }
        return (s, UNITS[idx].1);
    }
}

pub fn fmt_num(v: f64) -> String {
    let (n, unit) = fmt_big(v);
    format!("{n}{unit}")
}

pub fn fmt_ms(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{ms:.0}ms")
    } else if ms < 10_000.0 {
        format!("{:.2}s", ms / 1000.0)
    } else {
        format!("{:.1}s", ms / 1000.0)
    }
}

pub fn fmt_bytes(b: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b;
    let mut i = 0;
    while v.abs() >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    let (s, overflow) = sig3(v);
    if overflow && i + 1 < UNITS.len() {
        return format!("{} {}", sig3(v / 1024.0).0, UNITS[i + 1]);
    }
    format!("{s} {}", UNITS[i])
}

pub fn fmt_rate_bytes(bps: f64) -> String {
    format!("{}/s", fmt_bytes(bps))
}

pub fn fmt_age(d: chrono::Duration) -> String {
    let s = d.num_seconds().max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h", s / 3600)
    }
}

/// Cuts `s` to at most `max` terminal columns, ending with "…" when cut.
pub fn truncate_width(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > max - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn big_numbers() {
        assert_eq!(fmt_big(165.), ("165".into(), ""));
        assert_eq!(fmt_big(119.48), ("119".into(), ""));
        assert_eq!(fmt_big(11.72), ("11.7".into(), ""));
        assert_eq!(fmt_big(0.3137), ("0.31".into(), ""));
        assert_eq!(fmt_big(8_330_000.), ("8.33".into(), "M"));
        assert_eq!(fmt_big(12_900.), ("12.9".into(), "k"));
        assert_eq!(fmt_big(127_060_000.), ("127".into(), "M"));
        assert_eq!(fmt_big(100.), ("100".into(), ""));
        assert_eq!(fmt_big(0.), ("0".into(), ""));
        assert_eq!(fmt_big(999.6), ("1".into(), "k"));
        assert_eq!(fmt_big(2_500_000_000.), ("2.5".into(), "G"));
        assert_eq!(fmt_big(-12.), ("-12".into(), ""));
        assert_eq!(fmt_num(8_330_000.), "8.33M");
        assert_eq!(fmt_num(f64::NAN), "—");
    }

    #[test]
    fn durations() {
        assert_eq!(fmt_ms(119.48), "119ms");
        assert_eq!(fmt_ms(1500.), "1.50s");
        assert_eq!(fmt_ms(30020.), "30.0s");
    }

    #[test]
    fn bytes() {
        assert_eq!(fmt_bytes(11.72 * 1024f64.powi(3)), "11.7 GB");
        assert_eq!(fmt_bytes(12.9 * 1024f64.powi(2)), "12.9 MB");
        assert_eq!(fmt_bytes(512.), "512 B");
        assert_eq!(fmt_rate_bytes(12.9 * 1024f64.powi(2)), "12.9 MB/s");
    }

    #[test]
    fn ages() {
        assert_eq!(fmt_age(chrono::Duration::seconds(45)), "45s");
        assert_eq!(fmt_age(chrono::Duration::seconds(200)), "3m");
        assert_eq!(fmt_age(chrono::Duration::seconds(7300)), "2h");
        assert_eq!(fmt_age(chrono::Duration::seconds(-5)), "0s");
    }

    #[test]
    fn truncation_by_display_width() {
        let t = truncate_width("POST /api/import/upload", 10);
        assert!(t.width() <= 10);
        assert!(t.ends_with('…'));
        assert!(truncate_width("日本語テキスト", 5).width() <= 5);
        assert!(truncate_width(&("x".repeat(300) + "😀"), 20).width() <= 20);
        assert_eq!(truncate_width("ab", 5), "ab");
        assert_eq!(truncate_width("abc", 0), "");
        assert_eq!(truncate_width("abc", 1), "…");
        assert_eq!(truncate_width("abcd", 4), "abcd");
    }
}
