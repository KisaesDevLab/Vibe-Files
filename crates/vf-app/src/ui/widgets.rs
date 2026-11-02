//! Small shared widgets (UI_SPEC "egui mapping notes": implement once, reuse everywhere).

use egui::{Color32, CornerRadius, RichText, Stroke};

use crate::ui::theme;

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
