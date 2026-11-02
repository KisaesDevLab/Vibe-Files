//! Small shared widgets (UI_SPEC "egui mapping notes": implement once, reuse everywhere).

use egui::{Color32, CornerRadius, RichText, Stroke};

use crate::ui::theme;

/// A rounded label with a fill and text color (status, lock, warning pills).
pub fn pill(ui: &mut egui::Ui, text: &str, bg: Color32, fg: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| ui.label(RichText::new(text).small().color(fg)))
        .response
}

/// Document status pill (UI_SPEC status colors).
pub fn status_pill(ui: &mut egui::Ui, status: &str) -> egui::Response {
    let (bg, fg) = match status {
        "in_work" => (theme::WARN_BG, theme::WARN_FG),
        "final" => (theme::OK_BG, theme::OK_FG),
        "delivered" => (theme::SELECT_BG, theme::ACCENT_DEFAULT),
        _ => (theme::NEUTRAL_BG, theme::INK_SECONDARY),
    };
    pill(ui, crate::ui::format::status_label(status), bg, fg)
}

/// 11px uppercase section label.
pub fn section(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(
        RichText::new(text.to_uppercase())
            .small()
            .color(theme::INK_MUTED),
    );
}

pub fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text).monospace()
}

pub fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::INK_MUTED)
}

pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(Color32::WHITE))
        .fill(theme::ACCENT_DEFAULT)
        .stroke(Stroke::NONE)
        .min_size(egui::vec2(0.0, 32.0))
}

/// Error or warning text block.
pub fn banner(ui: &mut egui::Ui, text: &str, error: bool) {
    let (bg, fg) = if error {
        (theme::ERR_BG, theme::ERR_FG)
    } else {
        (theme::WARN_BG, theme::WARN_FG)
    };
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(fg));
        });
}

/// A label above a text field, linked for screen readers.
pub fn field(ui: &mut egui::Ui, label: &str, edit: egui::TextEdit<'_>) -> egui::Response {
    let l = ui.label(label);
    ui.add(edit).labelled_by(l.id)
}
