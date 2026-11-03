//! Small shared widgets (UI_SPEC "egui mapping notes": implement once, reuse everywhere).

use egui::{Color32, CornerRadius, RichText};

/// A rounded status pill.
pub fn pill(ui: &mut egui::Ui, text: &str, bg: Color32, fg: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| ui.label(RichText::new(text).small().color(fg)))
        .response
}

/// Document status: catalog value → (label, bg, fg) per UI_SPEC status pills.
pub fn status_style(status: &str) -> (&'static str, Color32, Color32) {
    match status {
        "in_work" => ("In work", rgb(0xFBEFD5), rgb(0x7A4F07)),
        "final" => ("Final", rgb(0xDDF2E4), rgb(0x1D5C38)),
        "delivered" => ("Delivered", rgb(0xE6EEF9), rgb(0x1F56A8)),
        _ => ("Received", rgb(0xEEF0F3), rgb(0x3F4753)),
    }
}

pub fn status_pill(ui: &mut egui::Ui, status: &str) -> egui::Response {
    let (label, bg, fg) = status_style(status);
    pill(ui, label, bg, fg)
}

/// Lock pill colors.
pub fn lock_pill(ui: &mut egui::Ui, user: &str) -> egui::Response {
    pill(
        ui,
        &format!("Locked · {user}"),
        rgb(0xFBEFD5),
        rgb(0x7A4F07),
    )
}

pub const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// `1.2 MB`, `84 KB`, `512 B`.
pub fn size(bytes: i64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b >= K * K * K {
        format!("{:.1} GB", b / (K * K * K))
    } else if b >= K * K {
        format!("{:.1} MB", b / (K * K))
    } else if b >= K {
        format!("{:.0} KB", b / K)
    } else {
        format!("{bytes} B")
    }
}

/// Storage time (RFC 3339 UTC) shown in local time, `2026-11-02 14:05`.
pub fn local_time(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(86_000), "84 KB");
        assert_eq!(size(1_258_291), "1.2 MB");
    }

    #[test]
    fn bad_time_shown_as_is() {
        assert_eq!(local_time("not a time"), "not a time");
        assert_eq!(local_time("2026-11-02T14:05:00Z").len(), 16);
    }
}
