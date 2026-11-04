//! Small shared widgets (UI_SPEC "egui mapping notes": implement once, reuse everywhere).

use egui::{Color32, CornerRadius, RichText};
use vf_fs::docmeta::STATUSES;

/// A rounded status pill.
pub fn pill(ui: &mut egui::Ui, text: &str, bg: Color32, fg: Color32) -> egui::Response {
    // Colors are given for the light theme; the dark one gets a dark tint (owner 2026-11-04).
    let (bg, fg) = crate::ui::theme::pill(bg, fg);
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| ui.label(RichText::new(text).small().color(fg)))
        .response
}

/// Document status: catalog value → (label, bg, fg). Labels come from
/// `vf_fs::docmeta::STATUSES`; colors are UI_SPEC's status pills.
pub fn status_style(status: &str) -> (&'static str, Color32, Color32) {
    let label = label_of(&STATUSES, Some(status), "Received");
    let (bg, fg) = match status {
        "in_work" => (rgb(0xFBEFD5), rgb(0x7A4F07)),
        "final" => (rgb(0xDDF2E4), rgb(0x1D5C38)),
        "delivered" => (rgb(0xE6EEF9), rgb(0x1F56A8)),
        _ => (rgb(0xEEF0F3), rgb(0x3F4753)),
    };
    (label, bg, fg)
}

/// Sort order of a status: its place in `STATUSES` (unknown values last).
pub fn status_rank(status: &str) -> usize {
    STATUSES
        .iter()
        .position(|(k, _)| *k == status)
        .unwrap_or(STATUSES.len())
}

/// The label for `key` in a `(key, label)` table, or `none`.
pub fn label_of(
    table: &[(&str, &'static str)],
    key: Option<&str>,
    none: &'static str,
) -> &'static str {
    key.and_then(|k| table.iter().find(|(kk, _)| *kk == k))
        .map(|(_, l)| *l)
        .unwrap_or(none)
}

/// The file name of a cabinet-relative path (forward slashes).
pub fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// A tax year for display; `None` is a permanent file.
pub fn year_label(y: Option<i32>) -> String {
    y.map(|y| y.to_string())
        .unwrap_or_else(|| "Permanent".into())
}

/// Small muted text: field captions, column headers, hints (UI_SPEC "labels 11px").
pub fn caption(text: impl Into<String>) -> RichText {
    RichText::new(text)
        .small()
        .color(crate::ui::theme::ink_muted())
}

/// The accent-filled primary button.
pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    let on_accent = if crate::ui::theme::is_dark() {
        crate::ui::theme::HEADER
    } else {
        Color32::WHITE
    };
    egui::Button::new(RichText::new(text).color(on_accent)).fill(crate::ui::theme::accent())
}

/// A fixed-size scrolling body for a modal. A modal that sizes itself to its content re-centers
/// over several frames and moves its buttons from under the pointer, so dialog bodies whose
/// content changes (previews, messages) must not change the modal's size.
pub fn fixed_body<R>(
    ui: &mut egui::Ui,
    id: &str,
    height: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .min_scrolled_height(height)
        .max_height(height)
        .auto_shrink([false, false])
        .show(ui, add)
        .inner
}

/// A fixed-height error line for a modal (empty when there is no message), for the same reason
/// as [`fixed_body`].
pub fn message_line(ui: &mut egui::Ui, msg: Option<&str>) {
    ui.add_sized(
        [ui.available_width(), 36.0],
        egui::Label::new(RichText::new(msg.unwrap_or("")).color(crate::ui::theme::error())).wrap(),
    );
}

/// What a modal dialog did this frame.
pub enum DialogOutcome<T> {
    Open,
    Cancelled,
    Done(T),
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
