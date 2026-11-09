//! Preview tab's side pane (owner 2026-11-04): the pages of the row last clicked, one at a time,
//! beside the batch table, so a file can be looked at before choosing its client or rule.

use std::path::{Path, PathBuf};

use egui::{Align, Layout, RichText};

use crate::pdf::Thumbnailer;
use crate::ui::fileview::FileViewer;
use crate::ui::theme::{file_name, pal};

/// Width of the pane, points.
pub const PANE_WIDTH: f32 = 380.0;

/// One page rendered at a time, this many pixels wide (sharp at 2x scaling).
const RENDER_WIDTH: u32 = 760;

/// The file shown and its current page.
pub struct PagePane {
    path: PathBuf,
    page: usize,
    renderer: Option<Thumbnailer>,
    /// Anything not a PDF that can be read: a spreadsheet, document, mail or picture (V27).
    viewer: Option<FileViewer>,
    /// The page drawn (index, texture) and the page asked for.
    shown: Option<(usize, egui::TextureHandle)>,
    asked: Option<usize>,
    error: Option<String>,
}

impl PagePane {
    fn open(ctx: &egui::Context, path: &Path, pdfium_dir: Option<PathBuf>) -> Self {
        let pdf = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        let renderer = pdfium_dir
            .filter(|_| pdf)
            .map(|d| Thumbnailer::new(ctx, d, path.to_path_buf(), RENDER_WIDTH));
        let viewer = (!pdf && crate::fileview::viewable(path)).then(|| FileViewer::open(ctx, path));
        Self {
            path: path.to_path_buf(),
            page: 0,
            renderer,
            viewer,
            shown: None,
            asked: None,
            error: None,
        }
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    /// The contents of a file that is not a PDF are read (tests).
    pub fn contents_ready(&self) -> bool {
        self.viewer.as_ref().is_some_and(|v| v.ready())
    }

    #[cfg(test)]
    pub fn page(&self) -> usize {
        self.page
    }

    #[cfg(test)]
    /// A page is drawn (tests).
    pub fn has_image(&self) -> bool {
        self.shown.as_ref().is_some_and(|(p, _)| *p == self.page)
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(r) = &self.renderer else { return };
        for (page, image) in r.finished() {
            match image {
                Ok(img) => {
                    let tex =
                        ctx.load_texture(format!("peek-{page}"), img, egui::TextureOptions::LINEAR);
                    self.shown = Some((page, tex));
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if self.asked != Some(self.page) {
            r.request(self.page);
            self.asked = Some(self.page);
        }
    }
}

/// Keeps `pane` on `path` (a new file starts at page 1).
pub fn follow(
    pane: &mut Option<PagePane>,
    ctx: &egui::Context,
    path: Option<&Path>,
    pdfium_dir: Option<PathBuf>,
) {
    match path {
        None => *pane = None,
        Some(p) if pane.as_ref().is_some_and(|x| x.path == p) => {}
        Some(p) => *pane = Some(PagePane::open(ctx, p, pdfium_dir)),
    }
}

/// Draws the pane. `pages`: the file's page count when known.
pub fn show(ui: &mut egui::Ui, pane: &mut Option<PagePane>, pages: Option<usize>, pdfium: bool) {
    let p = pal(ui);
    ui.set_width(PANE_WIDTH);
    ui.add_space(2.0);
    let Some(pane) = pane else {
        ui.label(RichText::new("Click a file name to see its pages here.").color(p.muted));
        return;
    };
    pane.poll(ui.ctx());
    if let Some(v) = &mut pane.viewer {
        ui.label(RichText::new(format!("Inside {}", file_name(v.path()))).strong())
            .on_hover_text(v.path().display().to_string());
        ui.label(
            RichText::new("Contents only. Double-click the file name to open it.")
                .small()
                .color(p.muted),
        );
        ui.add_space(4.0);
        let h = ui.available_height();
        v.show(ui, h);
        return;
    }
    // Worded unlike the row's own name and the Preview tab (one label each for automation).
    ui.label(RichText::new(format!("Pages of {}", file_name(&pane.path))).strong())
        .on_hover_text(pane.path.display().to_string());
    if pane.renderer.is_none() {
        let why = if pdfium {
            "No preview for this kind of file."
        } else {
            "Previews need pdfium.dll beside the program."
        };
        ui.label(RichText::new(why).color(p.muted));
        return;
    }
    let count = pages.unwrap_or(1).max(1);
    pane.page = pane.page.min(count - 1);
    ui.horizontal(|ui| {
        let prev = ui.add_enabled(pane.page > 0, egui::Button::new("Previous"));
        prev.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, pane.page > 0, "Previous page")
        });
        if prev.clicked() {
            pane.page -= 1;
        }
        ui.label(format!("Page {} of {count}", pane.page + 1));
        let more = pane.page + 1 < count;
        let next = ui.add_enabled(more, egui::Button::new("Next"));
        next.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, more, "Next page"));
        if next.clicked() {
            pane.page += 1;
        }
    });
    ui.add_space(4.0);
    if let Some(e) = &pane.error {
        ui.label(RichText::new(format!("Cannot show this page: {e}")).color(p.muted));
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("peek_page")
        .auto_shrink([false, false])
        .show(ui, |ui| match &pane.shown {
            Some((_, tex)) => {
                let size = tex.size_vec2();
                let w = ui.available_width().min(PANE_WIDTH);
                let h = size.y * w / size.x.max(1.0);
                let r = ui.add(
                    egui::Image::new((tex.id(), egui::vec2(w, h))).bg_fill(egui::Color32::WHITE),
                );
                // The page's edge, so a white page shows on a white pane.
                ui.painter().rect_stroke(
                    r.rect,
                    0.0,
                    egui::Stroke::new(1.0, pal(ui).border),
                    egui::StrokeKind::Outside,
                );
            }
            None => {
                ui.with_layout(Layout::top_down(Align::Center), |ui| {
                    ui.add_space(40.0);
                    ui.spinner();
                });
            }
        });
}
