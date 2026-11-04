//! The Help window (PA-25): the article list by section, a search box, and the article shown.
//! Opened by the header's **Help** button or F1, on the article about the tab shown.

use egui::{RichText, Stroke};

use crate::help::{self, Block, Section, Span};
use crate::ui::theme;

#[derive(Debug, Default)]
pub struct HelpState {
    pub open: bool,
    /// The article shown.
    pub current: Option<&'static str>,
    pub query: String,
    /// Articles shown before (Back).
    back: Vec<&'static str>,
    /// Scroll the article to the top on the next frame.
    to_top: bool,
}

impl HelpState {
    /// Opens Help on `id` (the article about the tab shown).
    pub fn open_at(&mut self, id: &str) {
        self.open = true;
        self.query.clear();
        self.go(id, false);
    }

    fn go(&mut self, id: &str, remember: bool) {
        let Some(a) = help::article(id) else { return };
        if remember
            && let Some(c) = self.current
            && c != a.id
        {
            self.back.push(c);
        }
        self.current = Some(a.id);
        self.to_top = true;
    }
}

pub fn show(ctx: &egui::Context, h: &mut HelpState) {
    if !h.open {
        return;
    }
    let mut open = true;
    let mut go: Option<&'static str> = None;
    egui::Window::new("Help")
        .open(&mut open)
        .default_size([860.0, 620.0])
        .min_size([560.0, 360.0])
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            egui::Panel::left("help-list")
                .resizable(true)
                .default_size(250.0)
                .show(ui, |ui| {
                    let l = ui.label("Search help");
                    ui.add(
                        egui::TextEdit::singleline(&mut h.query)
                            .hint_text("e.g. cover sheet, tape, password")
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(l.id);
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical()
                        .id_salt("help-list-scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if h.query.trim().is_empty() {
                                for s in Section::ALL {
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new(s.name()).strong().color(theme::ink_muted()),
                                    );
                                    for a in help::ARTICLES.iter().filter(|a| a.section == s) {
                                        let on = h.current == Some(a.id);
                                        if ui.selectable_label(on, a.title()).clicked() {
                                            go = Some(a.id);
                                        }
                                    }
                                }
                            } else {
                                let hits = help::search(&h.query);
                                if hits.is_empty() {
                                    ui.label(
                                        RichText::new("No article has all of those words.")
                                            .color(theme::ink_muted()),
                                    );
                                }
                                for a in hits {
                                    let on = h.current == Some(a.id);
                                    if ui.selectable_label(on, a.title()).clicked() {
                                        go = Some(a.id);
                                    }
                                    ui.label(
                                        RichText::new(short(&a.summary(), 110))
                                            .small()
                                            .color(theme::ink_muted()),
                                    );
                                    ui.add_space(4.0);
                                }
                            }
                        });
                });
            egui::CentralPanel::default().show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!h.back.is_empty(), egui::Button::new("Back"))
                        .clicked()
                        && let Some(b) = h.back.pop()
                    {
                        h.current = Some(b);
                        h.to_top = true;
                    }
                });
                ui.separator();
                let mut scroll = egui::ScrollArea::vertical()
                    .id_salt("help-article")
                    .auto_shrink([false, false]);
                if std::mem::take(&mut h.to_top) {
                    scroll = scroll.vertical_scroll_offset(0.0);
                }
                scroll.show(ui, |ui| {
                    ui.set_max_width(680.0);
                    match h.current.and_then(help::article) {
                        Some(a) => {
                            if let Some(id) = article(ui, a.text) {
                                go = Some(id);
                            }
                        }
                        None => {
                            ui.label("Choose an article on the left.");
                        }
                    }
                });
            });
        });
    if let Some(id) = go {
        h.go(id, true);
    }
    h.open = open;
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut t: String = s.chars().take(n).collect();
    t.push('…');
    t
}

/// Draws an article; the id of a link clicked.
fn article(ui: &mut egui::Ui, md: &str) -> Option<&'static str> {
    let mut go = None;
    for b in help::parse(md) {
        match b {
            Block::Title(t) => {
                ui.label(RichText::new(t).size(22.0).strong().color(theme::ink()));
                ui.add_space(6.0);
            }
            Block::Heading(t) => {
                ui.add_space(10.0);
                ui.label(RichText::new(t).size(17.0).strong().color(theme::ink()));
                ui.add_space(2.0);
            }
            Block::Sub(t) => {
                ui.add_space(6.0);
                ui.label(RichText::new(t).strong().color(theme::ink()));
            }
            Block::Para(s) => {
                line(ui, &s, &mut go);
                ui.add_space(6.0);
            }
            Block::Bullets(items) => {
                for s in items {
                    item(ui, "•  ".into(), &s, &mut go);
                }
                ui.add_space(6.0);
            }
            Block::Numbered(items) => {
                for (i, s) in items.iter().enumerate() {
                    item(ui, format!("{}.  ", i + 1), s, &mut go);
                }
                ui.add_space(6.0);
            }
            Block::Note(s) => {
                egui::Frame::new()
                    .fill(theme::soft(theme::accent()))
                    .stroke(Stroke::new(1.0, theme::border_pane()))
                    .corner_radius(4)
                    .inner_margin(8)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        line(ui, &s, &mut go);
                    });
                ui.add_space(6.0);
            }
        }
    }
    go
}

/// A list item: its mark, then the text, indented a little.
fn item(ui: &mut egui::Ui, mark: String, spans: &[Span], go: &mut Option<&'static str>) {
    let mut all = vec![Span::Text(mark)];
    all.extend_from_slice(spans);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.vertical(|ui| line(ui, &all, go));
    });
    ui.add_space(2.0);
}

/// One line of text with its marks; a link clicked sets `go`.
fn line(ui: &mut egui::Ui, spans: &[Span], go: &mut Option<&'static str>) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for s in spans {
            match s {
                Span::Text(t) => {
                    ui.label(RichText::new(t).color(theme::ink()));
                }
                Span::Bold(t) => {
                    ui.label(RichText::new(t).strong().color(theme::ink()));
                }
                Span::Code(t) => {
                    ui.label(RichText::new(t).code().color(theme::ink()));
                }
                Span::Link(t, id) => {
                    if ui.link(t).clicked() {
                        *go = help::article(id).map(|a| a.id);
                    }
                }
            }
        }
    });
}
