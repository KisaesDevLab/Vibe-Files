//! The header (wordmark, tabs with badges, profile dropdown) and the footer status bar that every
//! screen shares (UI_SPEC §2).

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Sense, Stroke};

use crate::app::Tab;
use crate::config::ThemeChoice;
use crate::ui::theme::{self, PillKind, pal, pill, strong};

/// Counts shown next to tab names.
#[derive(Debug, Default, Clone, Copy)]
pub struct Badges {
    /// Files waiting in the queue.
    pub preview: usize,
    /// Zip drafts (P07b).
    pub import: usize,
    /// Folders in Conflicts that are neither bound nor ignored (amber).
    pub conflicts: usize,
}

/// The header's profile dropdown.
pub struct ProfileChoice {
    pub names: Vec<String>,
    pub active: String,
    /// Why it can't be changed now (unsaved Rules edits), if so (X120).
    pub locked: Option<&'static str>,
}

/// Draws the header. Returns a profile the user picked, if any.
pub fn header(
    ui: &mut egui::Ui,
    tab: &mut Tab,
    badges: Badges,
    profile: ProfileChoice,
    theme_choice: &mut ThemeChoice,
) -> Option<String> {
    let p = pal(ui);
    let mut picked = None;
    egui::Panel::top("header")
        .exact_size(44.0)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(p.card)
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            let bottom = ui.max_rect().bottom();
            ui.horizontal_centered(|ui| {
                ui.label(strong("Vibe Filer").size(14.0).color(p.text));
                ui.label(RichText::new("Desktop").size(12.0).color(p.muted));
                ui.add_space(36.0);
                ui.spacing_mut().item_spacing.x = 6.0;
                for t in Tab::ALL {
                    tab_item(ui, tab, t, badges, bottom);
                    ui.add_space(14.0);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    theme_button(ui, theme_choice);
                    ui.add_space(10.0);
                    let mut active = profile.active.clone();
                    let combo = ui.add_enabled_ui(profile.locked.is_none(), |ui| {
                        egui::ComboBox::from_id_salt("header_profile")
                            .selected_text(active.clone())
                            .show_ui(ui, |ui| {
                                for name in &profile.names {
                                    ui.selectable_value(&mut active, name.clone(), name);
                                }
                            })
                    });
                    let label = ui.label(RichText::new("Profile").color(p.muted));
                    let resp = combo.inner.response.labelled_by(label.id);
                    if let Some(why) = profile.locked {
                        resp.on_disabled_hover_text(why);
                    }
                    if active != profile.active {
                        picked = Some(active);
                    }
                });
            });
        });
    picked
}

/// Sun (light), moon (dark) or a half-filled circle (follow Windows); a click cycles them (V1).
fn theme_button(ui: &mut egui::Ui, choice: &mut ThemeChoice) {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), Sense::click());
    let name = format!("Theme: {}", choice.label());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone()));
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(4), p.border.gamma_multiply(0.5));
    }
    let c = rect.center();
    let ink = p.text;
    let painter = ui.painter();
    match choice {
        ThemeChoice::Light => {
            painter.circle_filled(c, 4.5, ink);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::TAU / 8.0;
                let d = egui::vec2(a.cos(), a.sin());
                painter.line_segment([c + d * 7.0, c + d * 9.5], Stroke::new(1.5, ink));
            }
        }
        ThemeChoice::Dark => {
            painter.circle_filled(c, 7.0, ink);
            let bg = if resp.hovered() {
                p.card.lerp_to_gamma(p.border, 0.5)
            } else {
                p.card
            };
            painter.circle_filled(c + egui::vec2(3.5, -3.0), 6.0, bg);
        }
        ThemeChoice::System => {
            painter.circle_stroke(c, 7.0, Stroke::new(1.5, ink));
            let half: Vec<egui::Pos2> = (0..=16)
                .map(|k| {
                    let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 16.0;
                    c + egui::vec2(a.cos(), a.sin()) * 7.0
                })
                .collect();
            painter.add(egui::Shape::convex_polygon(half, ink, Stroke::NONE));
        }
    }
    let next = choice.next();
    if resp
        .on_hover_text(format!(
            "Theme: {} · click for {}",
            choice.label(),
            next.label()
        ))
        .clicked()
    {
        *choice = next;
    }
}

fn tab_item(ui: &mut egui::Ui, current: &mut Tab, tab: Tab, badges: Badges, bottom: f32) {
    let p = pal(ui);
    let selected = *current == tab;
    let text = if selected {
        strong(tab.label()).color(p.text)
    } else {
        RichText::new(tab.label()).color(p.text.gamma_multiply(0.85))
    };
    let resp = ui.add(egui::Button::new(text).frame(false).selected(selected));
    let mut rect = resp.rect;
    if resp.clicked() {
        *current = tab;
    }
    let (count, amber, what) = match tab {
        Tab::Preview => (badges.preview, false, "pending"),
        Tab::Import => (badges.import, false, "drafts"),
        Tab::Conflicts => (badges.conflicts, true, "unresolved"),
        _ => (0, false, ""),
    };
    if count > 0 {
        let fill = if amber { p.attention } else { p.accent };
        let galley = ui.painter().layout_no_wrap(
            count.to_string(),
            egui::FontId::new(11.0, egui::FontFamily::Name(theme::SEMIBOLD.into())),
            p.on_accent,
        );
        let size = egui::vec2((galley.size().x + 14.0).max(20.0), 18.0);
        let (badge_rect, badge) = ui.allocate_exact_size(size, Sense::click());
        ui.painter()
            .rect_filled(badge_rect, CornerRadius::same(9), fill);
        ui.painter().galley(
            badge_rect.center() - galley.size() / 2.0,
            galley,
            p.on_accent,
        );
        let name = format!("{}: {count} {what}", tab.label());
        badge.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, name.clone()));
        rect = rect.union(badge_rect);
        // Clicking the badge opens the tab too.
        if badge.clicked() {
            *current = tab;
        }
    }
    if selected {
        let y = bottom - 1.0;
        ui.painter().line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(2.0, p.accent),
        );
    }
}

/// What the footer shows.
pub struct Footer<'a> {
    pub root: Option<&'a std::path::Path>,
    pub inboxes: usize,
    pub clients: Option<usize>,
    pub relationships: usize,
    pub pdfium: bool,
    /// Per-screen text on the right (UI_SPEC §2).
    pub context: String,
}

pub fn footer(ui: &mut egui::Ui, f: Footer<'_>) {
    let p = pal(ui);
    egui::Panel::bottom("footer")
        .exact_size(28.0)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(p.card)
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                let small = |t: String| RichText::new(t).size(11.0).color(p.muted);
                match f.root {
                    Some(r) => {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.label(small("Root".into()));
                            ui.label(
                                theme::mono(r.display().to_string())
                                    .size(11.0)
                                    .color(p.text),
                            );
                        });
                    }
                    None => {
                        ui.label(small("No client root set".into()));
                    }
                }
                ui.label(small(match f.inboxes {
                    1 => "1 inbox watched".into(),
                    n => format!("{n} inboxes watched"),
                }));
                if let Some(n) = f.clients {
                    ui.label(small(format!(
                        "{} clients · {} relationship{}",
                        theme::thousands(n),
                        theme::thousands(f.relationships),
                        if f.relationships == 1 { "" } else { "s" }
                    )));
                }
                if f.pdfium {
                    pill(ui, PillKind::Ok, "pdfium loaded");
                } else {
                    pill(ui, PillKind::Err, "pdfium missing");
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(small(format!("v{}", env!("CARGO_PKG_VERSION"))));
                    if !f.context.is_empty() {
                        ui.label(small(f.context));
                    }
                });
            });
        });
}
