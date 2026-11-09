//! Draws what is inside a file that is not a PDF (V27): a spreadsheet as a grid with a tab per
//! sheet, a document or mail as text, a picture fitted to the pane. The file is read on its own
//! thread ([`crate::fileview::read`]); the window is woken once when it is ready.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

use egui::{RichText, text::LayoutJob};
use egui_extras::{Column, TableBuilder};

use crate::fileview::{Content, Doc, Sheet};

/// One file's contents for a preview pane.
pub struct FileViewer {
    path: PathBuf,
    rx: Option<mpsc::Receiver<Content>>,
    content: Option<Content>,
    texture: Option<egui::TextureHandle>,
    sheet: usize,
    /// Set when the pane moves on: a read still waiting for its turn is dropped.
    cancel: Arc<AtomicBool>,
}

impl Drop for FileViewer {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Files read at the same time, all panes together (QA: arrowing through a folder must not
/// start a read per file).
const READERS: usize = 2;
static BUSY: Mutex<usize> = Mutex::new(0);
static FREE: Condvar = Condvar::new();

/// One of the [`READERS`] places, given back when dropped (a read that panics too).
struct Turn;

impl Turn {
    /// Waits for a place; `None` when `cancel` is set first.
    fn take(cancel: &AtomicBool) -> Option<Turn> {
        let mut busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            if *busy < READERS {
                *busy += 1;
                return Some(Turn);
            }
            busy = FREE
                .wait_timeout(busy, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
        *busy = busy.saturating_sub(1);
        FREE.notify_one();
    }
}

impl FileViewer {
    /// Starts reading `path` in the background.
    pub fn open(ctx: &egui::Context, path: &Path) -> Self {
        let (tx, rx) = mpsc::channel();
        let (p, ctx) = (path.to_path_buf(), ctx.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("file-view".into())
            .spawn(move || {
                // The pane moved on before this file's turn: nothing to do.
                let Some(turn) = Turn::take(&stop) else {
                    return;
                };
                let content = crate::fileview::read(&p);
                drop(turn);
                if !stop.load(Ordering::Relaxed) && tx.send(content).is_ok() {
                    ctx.request_repaint();
                }
            });
        let (rx, content) = match spawned {
            Ok(_) => (Some(rx), None),
            Err(e) => (
                None,
                Some(Content::Nothing(format!("Cannot read this file: {e}"))),
            ),
        };
        Self {
            path: path.to_path_buf(),
            rx,
            content,
            texture: None,
            sheet: 0,
            cancel,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The contents have been read (tests).
    #[cfg(test)]
    pub fn ready(&self) -> bool {
        self.content.is_some()
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(c) => {
                    self.content = Some(c);
                    self.rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.content = Some(Content::Nothing("Cannot read this file.".into()));
                    self.rx = None;
                }
            }
        }
    }

    /// Draws the contents in the space left, at most `height` points tall.
    pub fn show(&mut self, ui: &mut egui::Ui, height: f32) {
        self.poll();
        let muted = ui.visuals().weak_text_color();
        let salt = egui::Id::NULL.with(("file_view", &self.path));
        match &mut self.content {
            None => {
                // Text, not a spinner: the reader wakes the window when it is done.
                ui.add_space(20.0);
                ui.label(RichText::new("Reading the file…").color(muted));
            }
            Some(Content::Nothing(why)) => {
                ui.add_space(8.0);
                ui.label(RichText::new(why.as_str()).color(muted));
            }
            Some(Content::Picture(pic)) => {
                if self.texture.is_none() {
                    let size = [pic.width as usize, pic.height as usize];
                    let img = egui::ColorImage::from_rgba_unmultiplied(size, &pic.rgba);
                    pic.rgba = Vec::new();
                    self.texture = Some(ui.ctx().load_texture(
                        format!("file-view-{}", self.path.display()),
                        img,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                if pic.pages > 1 {
                    ui.label(
                        RichText::new(format!(
                            "Page 1 of {}: open the file to see every page.",
                            pic.pages
                        ))
                        .color(muted),
                    );
                }
                if let Some(t) = &self.texture {
                    let size = t.size_vec2();
                    let w = ui.available_width();
                    let mut shown = egui::vec2(w, w * size.y / size.x.max(1.0));
                    if shown.y > height {
                        shown *= height / shown.y;
                    }
                    ui.add(egui::Image::new((t.id(), shown)));
                }
            }
            Some(Content::Text(doc)) => text(ui, doc, salt, height),
            Some(Content::Sheets(sheets)) => {
                self.sheet = self.sheet.min(sheets.len().saturating_sub(1));
                if sheets.len() > 1 {
                    egui::ScrollArea::horizontal()
                        .id_salt(salt.with("tabs"))
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (i, s) in sheets.iter().enumerate() {
                                    let name = if s.name.is_empty() {
                                        "(no name)"
                                    } else {
                                        &s.name
                                    };
                                    if ui.selectable_label(self.sheet == i, name).clicked() {
                                        self.sheet = i;
                                    }
                                }
                            });
                        });
                }
                let sheet = &sheets[self.sheet];
                if let Some(cut) = &sheet.cut {
                    ui.label(RichText::new(cut.as_str()).small().color(muted));
                }
                grid(ui, sheet, salt.with(self.sheet), height - 30.0);
            }
        }
    }
}

/// "A", "B" … "Z", "AA" … for 0-based column `n`.
fn column_name(mut n: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

fn grid(ui: &mut egui::Ui, sheet: &Sheet, salt: egui::Id, height: f32) {
    let muted = ui.visuals().weak_text_color();
    if sheet.rows.iter().all(|r| r.is_empty()) {
        ui.label(RichText::new("This sheet is empty.").color(muted));
        return;
    }
    let cols = sheet
        .rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .min(sheet.cols.max(1));
    // Each column as wide as its longest text in the first rows, within limits.
    let char_w = 7.0;
    let widths: Vec<f32> = (0..cols)
        .map(|c| {
            let longest = sheet
                .rows
                .iter()
                .take(200)
                .filter_map(|r| r.get(c))
                .map(|t| t.chars().count())
                .max()
                .unwrap_or(0);
            (longest as f32 * char_w + 12.0).clamp(44.0, 240.0)
        })
        .collect();
    let first_row = sheet.origin.0 + 1;
    let last_row = sheet.origin.0 + sheet.rows.len();
    let number_w = (last_row.to_string().len() as f32 * char_w + 10.0).max(28.0);
    let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    egui::ScrollArea::horizontal()
        .id_salt(salt.with("h"))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .id_salt(salt.with("table"))
                .striped(true)
                .resizable(true)
                .vscroll(true)
                .max_scroll_height(height.max(80.0))
                .auto_shrink([false, true])
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(number_w).resizable(false));
            for w in &widths {
                table = table.column(Column::initial(*w).at_least(24.0).clip(true));
            }
            table
                .header(row_h, |mut h| {
                    h.col(|_| {});
                    for c in 0..cols {
                        h.col(|ui| {
                            ui.label(
                                RichText::new(column_name(sheet.origin.1 + c))
                                    .small()
                                    .color(muted),
                            );
                        });
                    }
                })
                .body(|body| {
                    body.rows(row_h, sheet.rows.len(), |mut row| {
                        let i = row.index();
                        let cells = &sheet.rows[i];
                        row.col(|ui| {
                            ui.label(
                                RichText::new((first_row + i).to_string())
                                    .small()
                                    .color(muted),
                            );
                        });
                        for c in 0..cols {
                            row.col(|ui| {
                                if let Some(t) = cells.get(c).filter(|t| !t.is_empty()) {
                                    ui.add(egui::Label::new(t.as_str()).truncate());
                                }
                            });
                        }
                    });
                });
        });
}

fn text(ui: &mut egui::Ui, doc: &Doc, salt: egui::Id, height: f32) {
    let muted = ui.visuals().weak_text_color();
    let strong = ui.visuals().strong_text_color();
    let body = ui.visuals().text_color();
    if !doc.header.is_empty() {
        egui::Grid::new(salt.with("header"))
            .num_columns(2)
            .spacing([8.0, 2.0])
            .show(ui, |ui| {
                for (k, v) in &doc.header {
                    ui.label(RichText::new(k.as_str()).color(muted));
                    ui.add(egui::Label::new(RichText::new(v.as_str()).color(strong)).wrap());
                    ui.end_row();
                }
            });
        ui.separator();
    }
    if doc.paras.is_empty() {
        ui.label(RichText::new("No text in this file.").color(muted));
        return;
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    let heading_font = egui::FontId::proportional(font.size + 2.0);
    let mut job = LayoutJob::default();
    for (i, p) in doc.paras.iter().enumerate() {
        if i > 0 {
            job.append("\n", 0.0, egui::TextFormat::simple(font.clone(), body));
        }
        let format = if p.heading {
            egui::TextFormat::simple(heading_font.clone(), strong)
        } else {
            egui::TextFormat::simple(font.clone(), body)
        };
        job.append(&p.text, 0.0, format);
    }
    job.wrap.max_width = ui.available_width();
    egui::ScrollArea::vertical()
        .id_salt(salt.with("text"))
        .max_height(height.max(80.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.add(egui::Label::new(job).selectable(true).wrap());
            if doc.cut {
                ui.label(
                    RichText::new("The rest is not shown here: open the file to read it all.")
                        .color(muted),
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::column_name;

    #[test]
    fn column_letters_run_like_excel() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(701), "ZZ");
        assert_eq!(column_name(702), "AAA");
    }
}
