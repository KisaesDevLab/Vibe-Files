//! The Viewer tab, as much as P03 needs: one page of a PDF, previous / next, the search words
//! highlighted. Annotations, stamps and the rest arrive in P04.

use std::path::PathBuf;

use egui::{Color32, RichText};

use crate::ui::pages::{Renderer, Request};
use crate::ui::{theme, widgets};

/// Width pages are rendered at; drawn scaled to the tab.
const RENDER_WIDTH: u32 = 1400;
const HIGHLIGHT: Color32 = Color32::from_rgba_premultiplied(120, 100, 0, 90);

/// The open document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub doc_id: String,
    pub path: PathBuf,
    pub name: String,
    /// 0-based.
    pub page: usize,
    /// Words to highlight.
    pub terms: Vec<String>,
}

impl Doc {
    /// Document `doc_id` at cabinet path `rel`, opened at `page` with `terms` highlighted.
    pub fn at(
        root: &std::path::Path,
        doc_id: &str,
        rel: &str,
        page: usize,
        terms: Vec<String>,
    ) -> Doc {
        Doc {
            doc_id: doc_id.into(),
            path: root.join(rel),
            name: widgets::file_name(rel).into(),
            page,
            terms,
        }
    }
}

#[derive(Default)]
pub struct Viewer {
    pub doc: Option<Doc>,
    pub pages: Option<usize>,
    texture: Option<egui::TextureHandle>,
    hits: Vec<egui::Rect>,
    /// The page shown or being rendered (key).
    shown: Option<String>,
    renderer: Option<Renderer>,
    pub error: Option<String>,
    /// "Open in PDF app" pressed: the app runs it through Browse's shell.
    pub open_request: Option<PathBuf>,
}

impl Viewer {
    pub fn open(&mut self, doc: Doc) {
        self.doc = Some(doc);
        self.pages = None;
        self.shown = None;
        self.texture = None;
        self.hits.clear();
        self.error = None;
    }

    fn key(d: &Doc) -> String {
        format!("{}#{}", d.doc_id, d.page)
    }

    /// Asks for the current page if it is not the one shown, and takes finished renders.
    fn pump(&mut self, ctx: &egui::Context) {
        let Some(d) = &self.doc else { return };
        let r = self
            .renderer
            .get_or_insert_with(|| Renderer::new(ctx, crate::ui::intake::pdfium_dir(), true));
        let key = Self::key(d);
        for (k, res) in r.finished() {
            if k != key {
                continue;
            }
            match res {
                Ok(p) => {
                    self.pages = Some(p.pages);
                    self.hits = p.hits;
                    self.texture = Some(ctx.load_texture(
                        format!("viewer-{k}"),
                        p.image,
                        egui::TextureOptions::LINEAR,
                    ));
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if self.shown.as_deref() != Some(&key) {
            self.shown = Some(key.clone());
            r.request(Request {
                key,
                path: d.path.clone(),
                page: d.page,
                width: RENDER_WIDTH,
                terms: d.terms.clone(),
            });
        }
    }

    /// Whether the page on screen is the one asked for (tests).
    #[cfg(test)]
    pub fn ready(&self) -> bool {
        self.texture.is_some()
            && self
                .doc
                .as_ref()
                .is_some_and(|d| self.shown.as_deref() == Some(&Self::key(d)))
    }

    #[cfg(test)]
    pub fn hits(&self) -> &[egui::Rect] {
        &self.hits
    }
}

pub fn show(ui: &mut egui::Ui, v: &mut Viewer) {
    v.pump(ui.ctx());
    egui::CentralPanel::default().show(ui, |ui| {
        let Some(d) = v.doc.clone() else {
            ui.heading("Viewer");
            ui.label(widgets::caption(
                "Open a document from Search to see it here. Annotations and stamps arrive in P04.",
            ));
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(&d.name).strong());
            ui.separator();
            let pages = v.pages.unwrap_or(0);
            let prev = ui.add_enabled(d.page > 0, egui::Button::new("Previous page"));
            ui.label(if pages > 0 {
                format!("Page {} of {pages}", d.page + 1)
            } else {
                format!("Page {}", d.page + 1)
            });
            let next = ui.add_enabled(d.page + 1 < pages, egui::Button::new("Next page"));
            if let Some(doc) = &mut v.doc {
                if prev.clicked() {
                    doc.page -= 1;
                }
                if next.clicked() {
                    doc.page += 1;
                }
            }
            if !d.terms.is_empty() {
                ui.label(widgets::caption(format!(
                    "{} highlighted on this page",
                    v.hits.len()
                )));
            }
            ui.separator();
            if ui.button("Open in PDF app").clicked() {
                v.open_request = Some(d.path.clone());
            }
        });
        widgets::message_line(ui, v.error.as_deref());
        egui::ScrollArea::vertical().show(ui, |ui| match &v.texture {
            Some(tex) => {
                let avail = ui.available_width().min(1100.0);
                let size = tex.size_vec2();
                let scaled = size * (avail / size.x);
                let (rect, _) = ui.allocate_exact_size(scaled, egui::Sense::hover());
                ui.painter().image(
                    tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    (1.0, theme::BORDER_PANE),
                    egui::StrokeKind::Outside,
                );
                for h in &v.hits {
                    let r = egui::Rect::from_min_max(
                        rect.min + h.min.to_vec2() * rect.size(),
                        rect.min + h.max.to_vec2() * rect.size(),
                    )
                    .expand(1.5);
                    ui.painter().rect_filled(r, 2.0, HIGHLIGHT);
                }
            }
            None => {
                ui.spinner();
            }
        });
    });
}
