//! Header, tab bar and footer shared by every tab (UI_SPEC "Global chrome").

use egui::{Align, Frame, Layout, Margin, RichText, Sense, Stroke};

use crate::app::Tab;
use crate::ui::theme;

pub fn header(ui: &mut egui::Ui, cabinet: Option<&str>) {
    egui::Panel::top("header")
        .exact_size(44.0)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(theme::INK)
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(
                    RichText::new("Vibe Files")
                        .strong()
                        .color(egui::Color32::WHITE),
                );
                ui.label(
                    RichText::new(cabinet.unwrap_or("No cabinet open")).color(theme::ON_DARK_MUTED),
                );
            });
        });
}

/// Tab bar in the fixed order Browse · Intake · Search · Viewer · Sync · Admin.
pub fn tab_bar(ui: &mut egui::Ui, current: &mut Tab, intake_badge: usize) {
    egui::Panel::top("tabs")
        .exact_size(44.0)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(theme::PANE)
                .stroke(Stroke::new(1.0, theme::BORDER_PANE))
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                for t in Tab::ALL {
                    let label = match (t, intake_badge) {
                        (Tab::Intake, n) if n > 0 => format!("{} ({n})", t.name()),
                        _ => t.name().to_owned(),
                    };
                    let selected = *current == t;
                    let text = RichText::new(label).color(if selected {
                        theme::ACCENT_DEFAULT
                    } else {
                        theme::INK_SECONDARY
                    });
                    let resp = ui.add(egui::Button::selectable(selected, text).frame(false));
                    if selected {
                        let r = resp.rect;
                        ui.painter().hline(
                            r.x_range(),
                            ui.max_rect().bottom() - 1.0,
                            Stroke::new(2.0, theme::ACCENT_DEFAULT),
                        );
                    }
                    if resp.clicked() {
                        *current = t;
                    }
                    ui.add_space(12.0);
                }
            });
        });
}

pub fn footer(ui: &mut egui::Ui, segments: &[String], health: &str) {
    egui::Panel::bottom("footer")
        .exact_size(28.0)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(theme::FOOTER)
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                for s in segments {
                    ui.label(RichText::new(s).small().color(theme::INK_MUTED));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(health).small().color(theme::INK_MUTED));
                    let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                    ui.painter()
                        .circle_filled(r.center(), 4.0, theme::HEALTH_OK);
                });
            });
        });
}
