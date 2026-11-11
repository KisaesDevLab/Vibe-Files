//! Header, tab bar and footer shared by every tab (UI_SPEC "Global chrome").

use egui::{Align, Frame, Layout, Margin, RichText, Sense, Stroke};

use crate::app::Tab;
use crate::ui::theme;

/// The header; `text_size` is this PC's text size in percent, changed by its − / + buttons;
/// `help` is set when its **Help** button is pressed. `portal`: the cabinet has a Vibe Portal
/// set up, so **Portal** shows (left of Tip) and sets `open_portal` when pressed.
#[allow(clippy::too_many_arguments)]
pub fn header(
    ui: &mut egui::Ui,
    cabinet: Option<&str>,
    text_size: &mut u16,
    dark: &mut bool,
    help: &mut bool,
    tip: &mut bool,
    portal: bool,
    open_portal: &mut bool,
) {
    egui::Panel::top("vf-header")
        .exact_size(44.0)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(theme::HEADER)
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
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let sizes = crate::config::TEXT_SIZES;
                    let at = sizes
                        .iter()
                        .position(|s| *s >= *text_size)
                        .unwrap_or(sizes.len() - 1);
                    let white = |t: &str| RichText::new(t).color(egui::Color32::WHITE);
                    // The header is dark in both themes: its buttons get their own dark fill and
                    // a light outline (the light theme's white fill hid the white labels).
                    let header_button = |t: egui::RichText| {
                        egui::Button::new(t)
                            .fill(egui::Color32::from_rgb(0x2A, 0x30, 0x37))
                            .stroke(Stroke::new(1.0, theme::ON_DARK_MUTED))
                    };
                    // The owner's Ko-fi page (owner 2026-11-04).
                    if ui
                        .add(header_button(white("Tip VibeMyFirm")))
                        .on_hover_text("Support VibeMyFirm on Ko-fi (opens your browser)")
                        .clicked()
                    {
                        *tip = true;
                    }
                    ui.add_space(8.0);
                    // The portal's staff sign-in page (owner 2026-11-11), when one is set up.
                    if portal
                        && ui
                            .add(header_button(white("Portal")))
                            .on_hover_text(
                                "The client portal's staff page: sign in there (opens your browser)",
                            )
                            .clicked()
                    {
                        *open_portal = true;
                    }
                    if portal {
                        ui.add_space(8.0);
                    }
                    // Help (PA-25): the article about the tab shown; F1 too.
                    if ui
                        .add(header_button(white("Help")))
                        .on_hover_text("Help articles (F1)")
                        .clicked()
                    {
                        *help = true;
                    }
                    ui.add_space(8.0);
                    // Light / dark (owner 2026-11-04), kept on this PC.
                    if ui
                        .add(header_button(white(if *dark { "Light" } else { "Dark" })))
                        .on_hover_text(if *dark {
                            "Switch to the light theme"
                        } else {
                            "Switch to the dark theme"
                        })
                        .clicked()
                    {
                        *dark = !*dark;
                    }
                    ui.add_space(8.0);
                    if ui
                        .add_enabled(at + 1 < sizes.len(), header_button(white("A+")))
                        .on_hover_text("Larger text (Ctrl +)")
                        .clicked()
                    {
                        *text_size = sizes[at + 1];
                    }
                    ui.label(RichText::new(format!("{text_size}%")).color(theme::ON_DARK_MUTED));
                    if ui
                        .add_enabled(at > 0, header_button(white("A-")))
                        .on_hover_text("Smaller text (Ctrl -)")
                        .clicked()
                    {
                        *text_size = sizes[at - 1];
                    }
                    ui.label(RichText::new("Text size").color(theme::ON_DARK_MUTED));
                });
            });
        });
}

/// Tab bar in the fixed order Browse · Intake · Search · Viewer · Sync · Admin.
pub fn tab_bar(ui: &mut egui::Ui, current: &mut Tab, intake_badge: usize) {
    egui::Panel::top("vf-tabs")
        .exact_size(44.0)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(theme::pane())
                .stroke(Stroke::new(1.0, theme::border_pane()))
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
                        theme::accent()
                    } else {
                        theme::ink_secondary()
                    });
                    let resp = ui.add(egui::Button::selectable(selected, text).frame(false));
                    if selected {
                        let r = resp.rect;
                        ui.painter().hline(
                            r.x_range(),
                            ui.max_rect().bottom() - 1.0,
                            Stroke::new(2.0, theme::accent()),
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
    egui::Panel::bottom("vf-footer")
        .exact_size(28.0)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(theme::footer())
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                for s in segments {
                    ui.label(RichText::new(s).small().color(theme::ink_muted()));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(health).small().color(theme::ink_muted()));
                    let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                    ui.painter()
                        .circle_filled(r.center(), 4.0, theme::health_ok());
                });
            });
        });
}
