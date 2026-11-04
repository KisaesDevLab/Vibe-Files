//! Browse preview pane (PHASES PA-12): the focused document's pages at the top of the
//! inspector, read-only, rendered on its own thread (the Viewer's renderer) and cached by content
//! hash + page + width, so stepping back to a page seen before is instant and arrowing through a
//! folder never blocks a frame. "Open in Viewer" opens the full Viewer at the page shown.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use egui::{Color32, RichText, vec2};

use crate::ui::doc_render::{DocRenderer, Done, Job};
use crate::ui::{theme, widgets};

/// Page images kept (all documents together).
const KEPT: usize = 60;

#[derive(Default)]
pub struct Preview {
    renderer: Option<DocRenderer>,
    /// The document shown: (doc id, path, content hash).
    doc: Option<(String, PathBuf, String)>,
    generation: u64,
    pages: Option<Result<usize, String>>,
    /// 0-based page shown.
    pub page: usize,
    /// (hash, page, width) → image.
    cache: HashMap<(String, usize, u32), egui::TextureHandle>,
    order: VecDeque<(String, usize, u32)>,
    asked: Option<(u64, usize, u32)>,
}

/// What the pane asked for.
pub enum Act {
    None,
    /// "Open in Viewer" at this page.
    Open(usize),
    /// The document stepper: `-1` / `1`.
    Step(i32),
}

impl Preview {
    /// Shows `doc` (id, cabinet path, hash); the same one again changes nothing.
    pub fn set(&mut self, ctx: &egui::Context, root: &Path, id: &str, rel: &str, hash: &str) {
        if self
            .doc
            .as_ref()
            .is_some_and(|(i, _, h)| i == id && h == hash)
        {
            return;
        }
        let path = root.join(rel);
        self.generation += 1;
        self.pages = None;
        self.page = 0;
        self.asked = None;
        self.doc = Some((id.to_string(), path.clone(), hash.to_string()));
        if !is_pdf(rel) {
            return;
        }
        let r = self.renderer.get_or_insert_with(|| DocRenderer::new(ctx));
        r.send(Job::Open {
            generation: self.generation,
            path,
        });
    }

    pub fn clear(&mut self) {
        self.doc = None;
        self.pages = None;
    }

    fn pump(&mut self, ctx: &egui::Context) {
        let Some(r) = &self.renderer else { return };
        let hash = self.doc.as_ref().map(|d| d.2.clone()).unwrap_or_default();
        for d in r.finished() {
            match d {
                Done::Opened { generation, info } if generation == self.generation => {
                    self.pages = Some(info.map(|i| i.geoms.len()));
                }
                Done::Page {
                    generation,
                    page,
                    width,
                    image: Ok(img),
                    ..
                } if generation == self.generation => {
                    let key = (hash.clone(), page, width);
                    let t = ctx.load_texture(
                        format!("preview-{}-{page}-{width}", &hash[..hash.len().min(12)]),
                        img.image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.cache.insert(key.clone(), t);
                    self.order.push_back(key);
                    while self.order.len() > KEPT {
                        if let Some(old) = self.order.pop_front() {
                            self.cache.remove(&old);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn is_pdf(rel: &str) -> bool {
    rel.to_lowercase().ends_with(".pdf")
}

/// The pane. `at`: (position in the grid, rows in the grid) for the document stepper.
pub fn show(ui: &mut egui::Ui, p: &mut Preview, rel: &str, at: (usize, usize)) -> Act {
    p.pump(ui.ctx());
    let mut act = Act::None;
    let w = ui.available_width();
    egui::Frame::new()
        .fill(theme::canvas())
        .inner_margin(6)
        .corner_radius(4)
        .show(ui, |ui| {
            ui.set_width(w - 12.0);
            ui.horizontal(|ui| {
                let prev = ui
                    .add_enabled(at.0 > 0, egui::Button::new("Previous").small())
                    .on_hover_text("Previous document (Up arrow)");
                if prev.clicked() {
                    act = Act::Step(-1);
                }
                ui.label(widgets::caption(format!("{} of {}", at.0 + 1, at.1)));
                let next = ui
                    .add_enabled(at.0 + 1 < at.1, egui::Button::new("Next").small())
                    .on_hover_text("Next document (Down arrow)");
                if next.clicked() {
                    act = Act::Step(1);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if is_pdf(rel) && ui.small_button("Open in Viewer").clicked() {
                        act = Act::Open(p.page);
                    }
                });
            });
            if !is_pdf(rel) {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(widgets::file_name(rel)).monospace());
                    ui.label(widgets::caption(
                        "Opens in its own program (double-click or Enter).",
                    ));
                });
                ui.add_space(20.0);
                return;
            }
            let n = match &p.pages {
                None => {
                    // A note, not a spinner: the renderer wakes the window when the document is
                    // open, so nothing repaints while it waits.
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| ui.label(widgets::caption("Loading page…")));
                    ui.add_space(40.0);
                    return;
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e.as_str()).color(theme::error()));
                    return;
                }
                Some(Ok(n)) => *n,
            };
            let width = ((w - 12.0) * ui.ctx().pixels_per_point()).clamp(100.0, 1200.0) as u32;
            let width = (width / 32 + 1) * 32;
            let hash = p.doc.as_ref().map(|d| d.2.clone()).unwrap_or_default();
            let key = (hash, p.page, width);
            match p.cache.get(&key) {
                Some(t) => {
                    let size = t.size_vec2();
                    let shown = vec2(w - 12.0, (w - 12.0) * size.y / size.x.max(1.0));
                    ui.add(egui::Image::new((t.id(), shown)).bg_fill(Color32::WHITE));
                }
                None => {
                    if p.asked != Some((p.generation, p.page, width)) {
                        p.asked = Some((p.generation, p.page, width));
                        if let Some(r) = &p.renderer {
                            r.send(Job::Page {
                                generation: p.generation,
                                page: p.page,
                                width,
                                rotate: 0,
                                terms: vec![],
                            });
                        }
                    }
                    let (r, _) = ui.allocate_exact_size(
                        vec2(w - 12.0, (w - 12.0) * 1.29),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(r, 0.0, Color32::WHITE);
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("Page {}", p.page + 1),
                        egui::FontId::proportional(13.0),
                        theme::PAGE_INK_MUTED,
                    );
                }
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(p.page > 0, egui::Button::new("<").small())
                    .on_hover_text("Previous page")
                    .clicked()
                {
                    p.page -= 1;
                }
                ui.label(widgets::caption(format!("Page {} / {n}", p.page + 1)));
                if ui
                    .add_enabled(p.page + 1 < n, egui::Button::new(">").small())
                    .on_hover_text("Next page")
                    .clicked()
                {
                    p.page += 1;
                }
            });
        });
    act
}
