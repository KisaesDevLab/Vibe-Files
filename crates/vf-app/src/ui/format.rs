//! Display formatting: sizes, local times, status labels.

use chrono::{DateTime, Local};

pub fn size(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    if b < 1024.0 {
        format!("{bytes} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", b / 1024.0 / 1024.0)
    } else {
        format!("{:.1} GB", b / 1024.0 / 1024.0 / 1024.0)
    }
}

/// RFC 3339 UTC (storage) → `YYYY-MM-DD HH:MM` in local time. Unparsable text is shown as-is.
pub fn local_time(rfc3339: &str) -> String {
    DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| rfc3339.to_owned())
}

/// `HH:MM` local, for activity lists.
pub fn local_hm(rfc3339: &str) -> String {
    DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| t.with_timezone(&Local).format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| rfc3339.to_owned())
}

pub const STATUSES: [&str; 4] = ["received", "in_work", "final", "delivered"];

pub fn status_label(s: &str) -> &'static str {
    match s {
        "received" => "Received",
        "in_work" => "In work",
        "final" => "Final",
        "delivered" => "Delivered",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(188_416), "184 KB");
        assert_eq!(size(5 * 1024 * 1024 + 300_000), "5.3 MB");
    }

    #[test]
    fn bad_time_is_shown_raw() {
        assert_eq!(local_time("t"), "t");
        assert_eq!(local_time("2026-11-02T00:00:00Z").len(), 16);
    }
}
