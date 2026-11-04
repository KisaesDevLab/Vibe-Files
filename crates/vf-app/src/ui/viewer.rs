//! The Viewer tab (P04 step 1, UI_SPEC §4): thumbnails · canvas · tool panel.
//!
//! Canvas: continuous scroll or single page, zoom, fit width, rotate view, find in document,
//! go to page, drag to select and copy text. Only visible pages (± one) are rendered, on the
//! Viewer's own thread from the document in memory ([`crate::ui::doc_render`]).
//!
//! Tools: annotations, tickmarks and stamps are pending shapes on the canvas until **Save**
//! (a new version of the file, the old one kept: `vf_fs::content::replace`) or **Save as**
//! (a new document, original untouched: `vf_fs::content::add`). Redaction is step 5.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};
use vf_core::db::Connection;
use vf_pdf::annot::{self, Annotation, Kind, Placement, Preset, Tick};
use vf_pdf::layer::Geometry;
use vf_pdf::pii::{self, Kind as Pii};
use vf_pdf::redact::Redaction;

use crate::ui::doc_render::{DocInfo, DocRenderer, Done, Job, WordBox};
use crate::ui::{theme, widgets};

/// Screen pixels per PDF point at 100 % (UI_SPEC: a Letter page is 640 px wide at 100 %).
const BASE: f32 = 640.0 / 612.0;
const GAP: f32 = 14.0;
const THUMB_W: u32 = 88;
const TEXTURES_KEPT: usize = 24;
const CANVAS: Color32 = Color32::from_rgb(0xDE, 0xE2, 0xE8);
const HIT: Color32 = Color32::from_rgba_premultiplied(120, 100, 0, 90);
const PENDING_HL: Color32 = Color32::from_rgba_premultiplied(150, 130, 40, 110);

/// The open document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub doc_id: String,
    pub path: PathBuf,
    pub name: String,
    /// 0-based: the page shown (single page) or at the top of the view.
    pub page: usize,
    /// Words to highlight.
    pub terms: Vec<String>,
}

impl Doc {
    /// Document `doc_id` at cabinet path `rel`, opened at `page` with `terms` highlighted.
    pub fn at(root: &Path, doc_id: &str, rel: &str, page: usize, terms: Vec<String>) -> Doc {
        Doc {
            doc_id: doc_id.into(),
            path: root.join(rel),
            name: widgets::file_name(rel).into(),
            page,
            terms,
        }
    }
}

/// What a drag or click on the page does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Highlight,
    Underline,
    StrikeOut,
    Note,
    TextBox,
    Freehand,
    Arrow,
    Tickmark(Tick),
    /// Redact: draw a box.
    RedactBox,
    /// Redact: drag across words.
    RedactText,
}

impl Tool {
    const ANNOTATE: [(Tool, &'static str); 8] = [
        (Tool::Select, "Select text"),
        (Tool::Highlight, "Highlight"),
        (Tool::Underline, "Underline"),
        (Tool::StrikeOut, "Strikeout"),
        (Tool::Note, "Note"),
        (Tool::TextBox, "Text"),
        (Tool::Freehand, "Freehand"),
        (Tool::Arrow, "Arrow"),
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Panel {
    #[default]
    Annotate,
    Stamps,
    Redact,
}

const COLORS: [(&str, annot::Rgb); 4] = [
    ("Yellow", annot::YELLOW),
    ("Red", annot::RED),
    ("Blue", annot::ACCENT),
    ("Green", [0.11, 0.45, 0.25]),
];

/// A note or text box waiting for its words.
struct Typing {
    page: usize,
    /// Note: the point; text box: the box (user space).
    at: [f32; 4],
    note: bool,
    text: String,
}

struct Drag {
    page: usize,
    /// Display points.
    start: [f32; 2],
    points: Vec<[f32; 2]>,
}

/// What the catalog side of a save needs.
pub struct Env<'a> {
    pub conn: &'a mut Connection,
    pub root: &'a Path,
    pub device_id: &'a str,
    pub initials: &'a str,
    /// For "extract to another client".
    pub clients: &'a [vf_core::browse::ClientRow],
}

pub struct Viewer {
    pub doc: Option<Doc>,
    /// Another document asked for while annotations are unsaved: open it and drop them?
    pub ask_open: Option<Doc>,
    generation: u64,
    info: Option<DocInfo>,
    pub error: Option<String>,
    pub notice: Option<String>,
    renderer: Option<DocRenderer>,
    /// (page, width px, rotation) → page image; LRU.
    textures: HashMap<(usize, u32, u16), egui::TextureHandle>,
    order: VecDeque<(usize, u32, u16)>,
    requested: HashSet<(usize, u32, u16)>,
    /// Search-term boxes per page, fractions of the image.
    hits: HashMap<usize, Vec<Rect>>,
    thumbs: HashMap<usize, egui::TextureHandle>,
    thumbs_requested: HashSet<usize>,
    words: HashMap<usize, Vec<WordBox>>,
    words_requested: HashSet<usize>,
    pub zoom: f32,
    pub fit_width: bool,
    /// View rotation, degrees clockwise.
    pub rotate: u16,
    pub continuous: bool,
    goto: String,
    pub find: String,
    found: Option<(String, Vec<(usize, usize)>)>,
    find_at: usize,
    scroll_to: Option<usize>,
    pub tool: Tool,
    color: usize,
    pub panel: Panel,
    drag: Option<Drag>,
    typing: Option<Typing>,
    /// Annotations not saved yet.
    pub pending: Vec<Annotation>,
    preset: Preset,
    placement: Placement,
    /// "Open in PDF app" pressed: the app runs it through Browse's shell.
    pub open_request: Option<PathBuf>,
    /// A save changed the catalog: Browse reloads.
    pub catalog_changed: bool,
    /// Patterns auto-find looks for.
    pub pii_kinds: Vec<Pii>,
    /// Auto-find results, each with its checkbox.
    pub pii_found: Option<Vec<(pii::Found, bool)>>,
    /// Boxes drawn (or words selected) to redact.
    pub drawn: Vec<Redaction>,
    /// The "Pages…" window, while open.
    pub pages_dialog: Option<PagesDialog>,
}

/// The "Pages…" window's fields.
#[derive(Debug, Clone, Default)]
pub struct PagesDialog {
    /// Pages to act on, `1-3, 5`.
    pub pages: String,
    /// New order, `3, 1, 2`.
    pub order: String,
    /// Extract target: client id (`None` = this document's client).
    pub client: Option<String>,
    /// Extract target year: `2025`, `Permanent`, or empty for this document's.
    pub year: String,
}

impl Default for Viewer {
    fn default() -> Self {
        Viewer {
            doc: None,
            ask_open: None,
            generation: 0,
            info: None,
            error: None,
            notice: None,
            renderer: None,
            textures: HashMap::new(),
            order: VecDeque::new(),
            requested: HashSet::new(),
            hits: HashMap::new(),
            thumbs: HashMap::new(),
            thumbs_requested: HashSet::new(),
            words: HashMap::new(),
            words_requested: HashSet::new(),
            zoom: 1.0,
            fit_width: true,
            rotate: 0,
            continuous: true,
            goto: String::new(),
            find: String::new(),
            found: None,
            find_at: 0,
            scroll_to: None,
            tool: Tool::Select,
            color: 0,
            panel: Panel::Annotate,
            drag: None,
            typing: None,
            pending: Vec::new(),
            preset: Preset::Received,
            placement: Placement::TopRight,
            open_request: None,
            catalog_changed: false,
            pii_kinds: vec![Pii::Ssn, Pii::Ein],
            pii_found: None,
            drawn: Vec::new(),
            pages_dialog: None,
        }
    }
}

impl Viewer {
    pub fn open(&mut self, doc: Doc) {
        if !self.pending.is_empty() {
            // Unsaved annotations are never dropped unasked: the other document waits.
            self.ask_open = Some(doc);
            return;
        }
        self.open_now(doc);
    }

    /// Opens `doc`, dropping unsaved annotations (asked first by [`Viewer::open`]).
    fn open_now(&mut self, doc: Doc) {
        if !self.pending.is_empty() {
            self.notice = Some(format!(
                "{} unsaved annotations were discarded.",
                self.pending.len()
            ));
            self.pending.clear();
        } else {
            self.notice = None;
        }
        self.scroll_to = Some(doc.page);
        self.find = doc.terms.join(" ");
        self.pii_found = None;
        self.drawn.clear();
        self.doc = Some(doc);
        self.reload();
    }

    /// Reads the document again (after a save, or a new one opened).
    fn reload(&mut self) {
        self.generation += 1;
        self.info = None;
        self.error = None;
        self.textures.clear();
        self.order.clear();
        self.requested.clear();
        self.hits.clear();
        self.thumbs.clear();
        self.thumbs_requested.clear();
        self.words.clear();
        self.words_requested.clear();
        self.found = None;
        self.drag = None;
        self.typing = None;
        if let (Some(r), Some(d)) = (&self.renderer, &self.doc) {
            r.send(Job::Open {
                generation: self.generation,
                path: d.path.clone(),
            });
        }
    }

    fn pages(&self) -> usize {
        self.info.as_ref().map_or(0, |i| i.geoms.len())
    }

    /// Display size of page `i` in points, after the view rotation.
    fn page_size(&self, i: usize) -> (f32, f32) {
        let (w, h) = self
            .info
            .as_ref()
            .and_then(|inf| inf.geoms.get(i))
            .map_or((612.0, 792.0), Geometry::display);
        if self.rotate.is_multiple_of(180) {
            (w, h)
        } else {
            (h, w)
        }
    }

    fn terms(&self) -> Vec<String> {
        self.doc
            .as_ref()
            .map(|d| d.terms.clone())
            .unwrap_or_default()
    }

    /// Takes the worker's results.
    fn pump(&mut self, ctx: &egui::Context) {
        if self.renderer.is_none() {
            self.renderer = Some(DocRenderer::new(ctx));
            if self.doc.is_some() {
                self.reload();
            }
        }
        let done = self
            .renderer
            .as_ref()
            .map(DocRenderer::finished)
            .unwrap_or_default();
        for d in done {
            match d {
                Done::Opened { generation, info } if generation == self.generation => match info {
                    Ok(i) => self.info = Some(i),
                    Err(e) => self.error = Some(e),
                },
                Done::Page {
                    generation,
                    page,
                    width,
                    rotate,
                    image,
                } if generation == self.generation => match image {
                    Ok(r) if width == THUMB_W && rotate == u16::MAX => {
                        let t = ctx.load_texture(
                            format!("vthumb-{page}"),
                            r.image,
                            egui::TextureOptions::LINEAR,
                        );
                        self.thumbs.insert(page, t);
                    }
                    Ok(r) => {
                        let key = (page, width, rotate);
                        let t = ctx.load_texture(
                            format!("vpage-{page}-{width}-{rotate}"),
                            r.image,
                            egui::TextureOptions::LINEAR,
                        );
                        self.hits.insert(page, r.hits);
                        self.textures.insert(key, t);
                        self.order.push_back(key);
                        while self.order.len() > TEXTURES_KEPT {
                            if let Some(old) = self.order.pop_front() {
                                self.textures.remove(&old);
                                self.requested.remove(&old);
                            }
                        }
                    }
                    Err(e) => self.error = Some(e),
                },
                Done::Found {
                    generation,
                    text,
                    pages,
                } if generation == self.generation => {
                    if let Some(&(p, _)) = pages.first() {
                        self.goto_page(p);
                    } else {
                        self.notice = Some(format!("\"{text}\" is not in this document."));
                    }
                    self.found = Some((text, pages));
                    self.find_at = 0;
                }
                Done::Words {
                    generation,
                    page,
                    words,
                } if generation == self.generation => {
                    self.words.insert(page, words);
                }
                Done::Pii { generation, found } if generation == self.generation => {
                    self.pii_found = Some(
                        found
                            .into_iter()
                            .map(|f| {
                                let on = crate::redact::default_on(f.kind);
                                (f, on)
                            })
                            .collect(),
                    );
                }
                _ => {}
            }
        }
    }

    fn request_page(&mut self, page: usize, width: u32) {
        let key = (page, width, self.rotate);
        if self.textures.contains_key(&key) || !self.requested.insert(key) {
            return;
        }
        let terms = self.terms();
        if let Some(r) = &self.renderer {
            r.send(Job::Page {
                generation: self.generation,
                page,
                width,
                rotate: self.rotate,
                terms,
            });
        }
    }

    fn request_words(&mut self, page: usize) {
        if self.words.contains_key(&page) || !self.words_requested.insert(page) {
            return;
        }
        if let Some(r) = &self.renderer {
            r.send(Job::Words {
                generation: self.generation,
                page,
            });
        }
    }

    fn goto_page(&mut self, p: usize) {
        let last = self.pages().saturating_sub(1);
        if let Some(d) = &mut self.doc {
            d.page = p.min(last);
        }
        self.scroll_to = self.doc.as_ref().map(|d| d.page);
    }

    /// The texture showing the current page, if it is on screen yet.
    #[cfg(test)]
    fn current_texture(&self) -> Option<&egui::TextureHandle> {
        let p = self.doc.as_ref()?.page;
        self.textures
            .iter()
            .find(|((pg, _, r), _)| *pg == p && *r == self.rotate)
            .map(|(_, t)| t)
    }

    /// Whether the page asked for is on screen (tests).
    #[cfg(test)]
    pub fn ready(&self) -> bool {
        self.current_texture().is_some()
    }

    /// Search-term boxes on the current page (tests).
    #[cfg(test)]
    pub fn hits(&self) -> &[Rect] {
        self.doc
            .as_ref()
            .and_then(|d| self.hits.get(&d.page))
            .map_or(&[], Vec::as_slice)
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing

pub fn show(ui: &mut egui::Ui, v: &mut Viewer, env: &mut Env<'_>) {
    v.pump(ui.ctx());
    if let Some(next) = v.ask_open.clone() {
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("viewer-unsaved")).show(ui.ctx(), |ui| {
            ui.set_width(420.0);
            ui.heading(format!("{} unsaved annotations", v.pending.len()));
            ui.label(format!(
                "Opening {} drops them. Keep editing to save them first.",
                next.name
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Keep editing").clicked() {
                    answer = Some(false);
                }
                if ui.button("Discard and open").clicked() {
                    answer = Some(true);
                }
            });
        });
        match answer {
            Some(true) => {
                v.ask_open = None;
                v.open_now(next);
            }
            Some(false) => v.ask_open = None,
            None => {}
        }
    }
    let Some(doc) = v.doc.clone() else {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Viewer");
            ui.label(widgets::caption(
                "Open a document from Browse or Search to see it here.",
            ));
        });
        return;
    };
    toolbar(ui, v, env, &doc);
    footer(ui, v);
    if v.info.is_some() {
        thumbnails(ui, v);
        tool_panel(ui, v, env);
    }
    let initials = env.initials.to_string();
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(CANVAS))
        .show(ui, |ui| {
            widgets::message_line(ui, v.error.as_deref());
            if v.info.is_none() {
                ui.spinner();
                return;
            }
            canvas(ui, v, &initials);
        });
    typing_window(ui.ctx(), v, &initials);
    pages_window(ui.ctx(), v, env);
}

/// What the "Pages…" window asked for.
enum PageOp {
    Rotate(i64),
    Delete,
    Reorder,
    Extract,
    Split,
}

fn pages_window(ctx: &egui::Context, v: &mut Viewer, env: &mut Env<'_>) {
    let Some(d) = &mut v.pages_dialog else { return };
    let mut op = None;
    let mut close = false;
    let count = v.info.as_ref().map_or(0, |i| i.geoms.len());
    egui::Window::new("Pages")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(widgets::caption(format!("{count} pages. Pages like 1-3, 5.")));
            let l = ui.label("Pages");
            ui.add(egui::TextEdit::singleline(&mut d.pages).desired_width(200.0))
                .labelled_by(l.id);
            ui.horizontal(|ui| {
                if ui.button("Rotate left").clicked() {
                    op = Some(PageOp::Rotate(-90));
                }
                if ui.button("Rotate right").clicked() {
                    op = Some(PageOp::Rotate(90));
                }
                if ui.button("Delete pages").clicked() {
                    op = Some(PageOp::Delete);
                }
            });
            ui.separator();
            let l = ui.label("New order");
            ui.add(
                egui::TextEdit::singleline(&mut d.order)
                    .hint_text("e.g. 3, 1, 2")
                    .desired_width(200.0),
            )
            .labelled_by(l.id);
            if ui.button("Reorder").clicked() {
                op = Some(PageOp::Reorder);
            }
            ui.separator();
            ui.label(RichText::new("Extract the pages to a new document").strong());
            let name = |id: &Option<String>| {
                id.as_ref()
                    .and_then(|id| env.clients.iter().find(|c| &c.id == id))
                    .map_or("This document's client".to_string(), |c| {
                        format!("{} ({})", c.name, c.ext_id)
                    })
            };
            egui::ComboBox::new("vx-client", "Client")
                .selected_text(name(&d.client))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.client, None, "This document's client");
                    for c in env.clients {
                        ui.selectable_value(
                            &mut d.client,
                            Some(c.id.clone()),
                            format!("{} ({})", c.name, c.ext_id),
                        );
                    }
                });
            let l = ui.label("Year");
            ui.add(
                egui::TextEdit::singleline(&mut d.year)
                    .hint_text("this document's")
                    .desired_width(100.0),
            )
            .labelled_by(l.id);
            ui.horizontal(|ui| {
                if ui.button("Extract").clicked() {
                    op = Some(PageOp::Extract);
                }
                if ui
                    .button("Split into documents")
                    .on_hover_text("Each range in Pages becomes its own document (1-2, 3-5); this one is kept.")
                    .clicked()
                {
                    op = Some(PageOp::Split);
                }
            });
            ui.separator();
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    if close {
        v.pages_dialog = None;
        return;
    }
    if let Some(op) = op {
        let d = v.pages_dialog.clone().unwrap_or_default();
        match run_page_op(v, env, &d, op, count) {
            Ok(msg) => {
                v.notice = Some(msg);
                v.catalog_changed = true;
                v.pages_dialog = None;
            }
            Err(e) => v.error = Some(e),
        }
    }
}

fn run_page_op(
    v: &mut Viewer,
    env: &mut Env<'_>,
    d: &PagesDialog,
    op: PageOp,
    count: usize,
) -> Result<String, String> {
    let doc = v.doc.clone().ok_or("no document")?;
    let pages = || vf_pdf::pages::parse_ranges(&d.pages, count);
    // Edits of this document: a new version.
    let edit = |v: &mut Viewer,
                env: &mut Env<'_>,
                what: &str,
                f: &dyn Fn(&Path, &Path) -> Result<(), String>|
     -> Result<String, String> {
        let tmp = vf_fs::content::tmp_file(env.root, "pdf")?;
        f(&doc.path, &tmp).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        let change = vf_fs::content::Change {
            label: "pages",
            created_by: "pages",
            reason: "before page edit",
            detail: serde_json::json!({ "edit": what }),
            extra: vec![],
        };
        match vf_fs::content::replace(env.conn, env.root, env.device_id, &doc.doc_id, &tmp, change)?
        {
            Ok(_) => {
                v.reload();
                Ok(format!("{what}; the previous version is kept."))
            }
            Err(why) => Err(format!("Not changed: {why}.")),
        }
    };
    match op {
        PageOp::Rotate(deg) => {
            let p = pages()?;
            edit(v, env, &format!("Turned {} pages", p.len()), &|a, b| {
                vf_pdf::pages::rotate(a, b, &p, deg)
            })
        }
        PageOp::Delete => {
            let p = pages()?;
            edit(v, env, &format!("Deleted {} pages", p.len()), &|a, b| {
                vf_pdf::pages::delete(a, b, &p)
            })
        }
        PageOp::Reorder => {
            let order = vf_pdf::pages::parse_ranges(&d.order, count)?;
            if order.len() != count {
                return Err(format!("List all {count} pages in their new order."));
            }
            edit(v, env, "Pages reordered", &|a, b| {
                vf_pdf::pages::reorder(a, b, &order)
            })
        }
        PageOp::Extract => {
            let p = pages()?;
            let added = extract(env, &doc, &[p], d)?;
            Ok(format!("Extracted to {}.", added.join(", ")))
        }
        PageOp::Split => {
            let ranges: Vec<Vec<usize>> = d
                .pages
                .split(',')
                .filter(|r| !r.trim().is_empty())
                .map(|r| vf_pdf::pages::parse_ranges(r, count))
                .collect::<Result<_, _>>()?;
            let added = extract(env, &doc, &ranges, d)?;
            Ok(format!(
                "Split into {} documents: {}.",
                added.len(),
                added.join(", ")
            ))
        }
    }
}

/// Each page list in `parts` becomes a new document (Filer's `split_pdf`), in the chosen
/// client's folder (`Other/<year>`) or beside this one. This document is not changed.
fn extract(
    env: &mut Env<'_>,
    doc: &Doc,
    parts: &[Vec<usize>],
    d: &PagesDialog,
) -> Result<Vec<String>, String> {
    let row: (
        String,
        Option<String>,
        Option<String>,
        Option<i32>,
        Option<String>,
    ) = env
        .conn
        .query_row(
            "SELECT rel_path, client_id, doc_type_id, tax_year, source FROM document WHERE id = ?1",
            [&doc.doc_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map_err(|e| e.to_string())?;
    let (rel, own_client, doc_type, own_year, source) = row;
    let year = match d.year.trim() {
        "" => own_year,
        t if t.eq_ignore_ascii_case("permanent") => None,
        t => Some(
            t.parse::<i32>()
                .map_err(|_| format!("\"{t}\" is not a year"))?,
        ),
    };
    let client = d.client.clone().or(own_client.clone());
    let stem = doc.name.trim_end_matches(".pdf").to_string();
    // Same client and year: beside this one; otherwise that client's Other/<year> folder.
    let dir = if client == own_client && d.year.trim().is_empty() {
        rel.rsplit_once('/')
            .map(|(dir, _)| dir.to_string())
            .unwrap_or_default()
    } else {
        let base = client
            .as_ref()
            .and_then(|c| env.clients.iter().find(|x| &x.id == c))
            .and_then(|c| c.folder_rel.clone())
            .unwrap_or_default();
        let sub = year.map_or("Other".to_string(), |y| format!("Other/{y}"));
        if base.is_empty() {
            sub
        } else {
            format!("{base}/{sub}")
        }
    };
    let mut names = Vec::new();
    for (n, pages) in parts.iter().enumerate() {
        let tmp = vf_fs::content::tmp_file(env.root, "pdf")?;
        cut(&doc.path, pages, &tmp, env.root)?;
        let label = if parts.len() > 1 {
            format!("{stem} (part {})", n + 1)
        } else {
            format!("{stem} p{}", page_label(pages))
        };
        let rel_path = if dir.is_empty() {
            format!("{label}.pdf")
        } else {
            format!("{dir}/{label}.pdf")
        };
        let added = vf_fs::content::add(
            env.conn,
            env.root,
            env.device_id,
            &tmp,
            vf_fs::content::NewDoc {
                rel_path,
                client_id: client.clone(),
                doc_type_id: doc_type.clone(),
                tax_year: year,
                source: source.clone(),
                tags: vec![format!("extracted-from:{}", doc.doc_id)],
                label: "extract_pages".into(),
                detail: serde_json::json!({ "from": doc.doc_id, "pages": pages.iter().map(|p| p + 1).collect::<Vec<_>>() }),
            },
        )?;
        names.push(widgets::file_name(&added.rel_path).to_string());
    }
    Ok(names)
}

/// Pages `order` (0-based, in that order) of `src` into `out`: Filer's `split_pdf` keeps only
/// what those pages use; a reorder puts them in the order asked.
fn cut(src: &Path, order: &[usize], out: &Path, root: &Path) -> Result<(), String> {
    let mut sorted = order.to_vec();
    sorted.sort_unstable();
    let lo = *sorted.first().ok_or("no pages")?;
    let hi = *sorted.last().ok_or("no pages")?;
    let span = vf_fs::content::tmp_file(root, "pdf")?;
    filer_pdf::pdf::split_pdf(
        src,
        std::slice::from_ref(&(lo..hi + 1)),
        std::slice::from_ref(&span),
    )?;
    let n = hi + 1 - lo;
    let keep: Vec<usize> = order.iter().map(|p| p - lo).collect();
    let r = if keep == (0..n).collect::<Vec<_>>() {
        std::fs::rename(&span, out).map_err(|e| e.to_string())
    } else {
        // Drop the pages in the span that were not asked for, then put the rest in order.
        let tmp2 = vf_fs::content::tmp_file(root, "pdf")?;
        let gone: Vec<usize> = (0..n).filter(|p| !keep.contains(p)).collect();
        let r = if gone.is_empty() {
            std::fs::copy(&span, &tmp2)
                .map(|_| ())
                .map_err(|e| e.to_string())
        } else {
            vf_pdf::pages::delete(&span, &tmp2, &gone)
        };
        let r = r.and_then(|()| {
            // After deleting, page i of tmp2 is the i-th kept page in original order.
            let mut kept_sorted: Vec<usize> = keep.clone();
            kept_sorted.sort_unstable();
            let new_order: Vec<usize> = keep
                .iter()
                .map(|k| kept_sorted.iter().position(|x| x == k).unwrap_or(0))
                .collect();
            vf_pdf::pages::reorder(&tmp2, out, &new_order)
        });
        let _ = std::fs::remove_file(&tmp2);
        r
    };
    let _ = std::fs::remove_file(&span);
    r
}

/// `1-3` / `2` / `1-2,5` for a file name.
fn page_label(pages: &[usize]) -> String {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &p in pages {
        match runs.last_mut() {
            Some((_, b)) if *b + 1 == p => *b = p,
            _ => runs.push((p, p)),
        }
    }
    runs.iter()
        .map(|(a, b)| {
            if a == b {
                format!("{}", a + 1)
            } else {
                format!("{}-{}", a + 1, b + 1)
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn toolbar(ui: &mut egui::Ui, v: &mut Viewer, env: &mut Env<'_>, doc: &Doc) {
    egui::Panel::top("vf-viewer-bar")
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(theme::PANE)
                .inner_margin(egui::Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&doc.name).monospace().strong());
                if !v.pending.is_empty() {
                    widgets::pill(
                        ui,
                        &format!("{} unsaved", v.pending.len()),
                        Color32::from_rgb(0xFF, 0xF1, 0xC9),
                        Color32::from_rgb(0x7A, 0x52, 0x00),
                    );
                }
                ui.separator();
                if ui.button("−").on_hover_text("Zoom out").clicked() {
                    v.zoom = (v.zoom / 1.25).max(0.25);
                    v.fit_width = false;
                }
                ui.label(if v.fit_width {
                    "Fit".to_string()
                } else {
                    format!("{:.0}%", v.zoom * 100.0)
                });
                if ui.button("+").on_hover_text("Zoom in").clicked() {
                    v.zoom = (v.zoom * 1.25).min(6.0);
                    v.fit_width = false;
                }
                ui.toggle_value(&mut v.fit_width, "Fit width");
                if ui.button("Rotate view").clicked() {
                    v.rotate = (v.rotate + 90) % 360;
                }
                ui.checkbox(&mut v.continuous, "Continuous");
                ui.separator();
                let l = ui.label("Find");
                let r = ui
                    .add(egui::TextEdit::singleline(&mut v.find).desired_width(160.0))
                    .labelled_by(l.id);
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    run_find(v);
                }
                if let Some((_, pages)) = v.found.clone()
                    && !pages.is_empty()
                {
                    let n = pages.len();
                    if ui.small_button("Previous match").clicked() {
                        v.find_at = (v.find_at + n - 1) % n;
                        v.goto_page(pages[v.find_at].0);
                    }
                    if ui.small_button("Next match").clicked() {
                        v.find_at = (v.find_at + 1) % n;
                        v.goto_page(pages[v.find_at].0);
                    }
                    let total: usize = pages.iter().map(|p| p.1).sum();
                    ui.label(widgets::caption(format!("{total} matches on {n} pages")));
                }
                ui.separator();
                let pages = v.pages();
                if ui
                    .add_enabled(doc.page > 0, egui::Button::new("Previous page"))
                    .clicked()
                {
                    v.goto_page(doc.page - 1);
                }
                let l = ui.label("Go to");
                let r = ui
                    .add(
                        egui::TextEdit::singleline(&mut v.goto)
                            .hint_text((doc.page + 1).to_string())
                            .desired_width(36.0),
                    )
                    .labelled_by(l.id);
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Ok(n) = v.goto.trim().parse::<usize>() {
                        v.goto_page(n.saturating_sub(1));
                    }
                    v.goto.clear();
                }
                ui.label(if pages > 0 {
                    format!("Page {} of {pages}", doc.page + 1)
                } else {
                    format!("Page {}", doc.page + 1)
                });
                if ui
                    .add_enabled(doc.page + 1 < pages, egui::Button::new("Next page"))
                    .clicked()
                {
                    v.goto_page(doc.page + 1);
                }
                ui.separator();
                let clean = v.pending.is_empty();
                if ui
                    .add_enabled(clean, egui::Button::new("Flatten"))
                    .on_hover_text("Burn annotations and form fields into the pages (a new version; the current one is kept).")
                    .on_disabled_hover_text("Save or discard the unsaved annotations first.")
                    .clicked()
                {
                    convert(v, env, Convert::Flatten);
                }
                if ui
                    .add_enabled(clean, egui::Button::new("PDF/A"))
                    .on_hover_text("Make this file PDF/A-2b for archiving (a new version; the current one is kept). Pages whose fonts are not embedded become images with a searchable text layer.")
                    .on_disabled_hover_text("Save or discard the unsaved annotations first.")
                    .clicked()
                {
                    convert(v, env, Convert::PdfA);
                }
                if ui
                    .add_enabled(clean, egui::Button::new("Pages…"))
                    .on_hover_text("Rotate, delete, reorder, extract or split pages.")
                    .on_disabled_hover_text("Save or discard the unsaved annotations first.")
                    .clicked()
                {
                    v.pages_dialog = Some(PagesDialog {
                        pages: (doc.page + 1).to_string(),
                        ..PagesDialog::default()
                    });
                }
                if ui
                    .add_enabled(clean, egui::Button::new("Compress"))
                    .on_hover_text("Bring scanned images down to 200 dpi and re-encode them (a new version; the current one is kept).")
                    .on_disabled_hover_text("Save or discard the unsaved annotations first.")
                    .clicked()
                {
                    convert(v, env, Convert::Compress);
                }
                if ui.button("Open in PDF app").clicked() {
                    v.open_request = Some(doc.path.clone());
                }
                let can = !v.pending.is_empty();
                if ui
                    .add_enabled(can, egui::Button::new("Save as…"))
                    .on_hover_text("A new document beside this one; this file is not changed.")
                    .clicked()
                {
                    save(v, env, true);
                }
                if ui
                    .add_enabled(can, widgets::primary_button("Save"))
                    .on_hover_text("A new version of this file; the current one is kept.")
                    .clicked()
                {
                    save(v, env, false);
                }
            });
            if let Some(n) = &v.notice {
                ui.label(widgets::caption(n));
            }
        });
}

fn run_find(v: &mut Viewer) {
    let text = v.find.trim().to_string();
    if let Some(d) = &mut v.doc {
        d.terms = text.split_whitespace().map(str::to_string).collect();
    }
    // Pages render again with the new words marked.
    v.textures.clear();
    v.order.clear();
    v.requested.clear();
    v.hits.clear();
    if let Some(r) = &v.renderer {
        r.send(Job::Find {
            generation: v.generation,
            text,
        });
    }
}

fn footer(ui: &mut egui::Ui, v: &Viewer) {
    egui::Panel::bottom("vf-viewer-foot")
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(theme::FOOTER)
                .inner_margin(egui::Margin::symmetric(12, 4)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(i) = &v.info {
                    ui.label(widgets::caption(format!(
                        "{} pages · {}",
                        i.geoms.len(),
                        widgets::size(i.bytes as i64)
                    )));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(widgets::caption(
                        "Save = new version of this file (current one kept) · Save as = new document, original untouched",
                    ));
                });
            });
        });
}

fn thumbnails(ui: &mut egui::Ui, v: &mut Viewer) {
    egui::Panel::left("vf-viewer-thumbs")
        .exact_size(112.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(theme::PANE).inner_margin(8))
        .show(ui, |ui| {
            let n = v.pages();
            let current = v.doc.as_ref().map_or(0, |d| d.page);
            let row_h = 128.0;
            egui::ScrollArea::vertical()
                .id_salt("vthumbs")
                .auto_shrink(false)
                .show_rows(ui, row_h, n, |ui, range| {
                    for i in range {
                        if !v.thumbs.contains_key(&i)
                            && v.thumbs_requested.insert(i)
                            && let Some(r) = &v.renderer
                        {
                            // `rotate: u16::MAX` marks a thumbnail (always upright).
                            r.send(Job::Page {
                                generation: v.generation,
                                page: i,
                                width: THUMB_W,
                                rotate: u16::MAX,
                                terms: vec![],
                            });
                        }
                        let (rect, resp) = ui
                            .allocate_exact_size(vec2(THUMB_W as f32, row_h - 8.0), Sense::click());
                        let p = ui.painter();
                        if let Some(t) = v.thumbs.get(&i) {
                            let s = t.size_vec2() * (THUMB_W as f32 / t.size_vec2().x);
                            let r =
                                Rect::from_min_size(rect.min, s.min(rect.size() - vec2(0.0, 14.0)));
                            p.image(t.id(), r, uv(), Color32::WHITE);
                            let stroke = if i == current {
                                Stroke::new(2.0, theme::ACCENT_DEFAULT)
                            } else {
                                Stroke::new(1.0, theme::BORDER_PANE)
                            };
                            p.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Outside);
                        } else {
                            p.rect_filled(rect.shrink2(vec2(0.0, 8.0)), 2.0, theme::APP_BG);
                        }
                        p.text(
                            pos2(rect.center().x, rect.max.y - 6.0),
                            egui::Align2::CENTER_BOTTOM,
                            (i + 1).to_string(),
                            egui::FontId::proportional(11.0),
                            theme::INK_MUTED,
                        );
                        if resp.clicked() {
                            v.goto_page(i);
                        }
                    }
                });
        });
}

fn uv() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

/// Pixels per point at the current zoom, given the canvas width.
fn scale(v: &Viewer, avail: f32) -> f32 {
    if v.fit_width {
        let widest = (0..v.pages().max(1))
            .map(|i| v.page_size(i).0)
            .fold(0.0_f32, f32::max)
            .max(1.0);
        ((avail - 48.0) / widest).max(0.1)
    } else {
        v.zoom * BASE
    }
}

fn canvas(ui: &mut egui::Ui, v: &mut Viewer, initials: &str) {
    let s = scale(v, ui.available_width());
    let ppp = ui.ctx().pixels_per_point();
    let n = v.pages();
    let shown: Vec<usize> = if v.continuous {
        (0..n).collect()
    } else {
        v.doc
            .as_ref()
            .map(|d| vec![d.page.min(n.saturating_sub(1))])
            .unwrap_or_default()
    };
    // Layout: each page's top in content coordinates.
    let mut tops = Vec::with_capacity(shown.len());
    let mut y = GAP;
    let mut widest = 0.0_f32;
    for &i in &shown {
        tops.push(y);
        let (w, h) = v.page_size(i);
        y += h * s + GAP;
        widest = widest.max(w * s);
    }
    let total = vec2(widest + 2.0 * GAP, y);
    let mut area = egui::ScrollArea::both()
        .id_salt("vcanvas")
        .auto_shrink(false);
    let jumped = v.scroll_to.take();
    if let Some(p) = jumped
        && let Some(k) = shown.iter().position(|&i| i == p)
    {
        area = area.vertical_scroll_offset(tops[k] - GAP);
    }
    let editable = v.rotate == 0;
    area.show_viewport(ui, |ui, viewport| {
        ui.set_min_size(total);
        let origin = ui.min_rect().min;
        let avail = ui.available_width().max(total.x);
        let mut top_page = None;
        for (k, &i) in shown.iter().enumerate() {
            let (w, h) = v.page_size(i);
            let size = vec2(w * s, h * s);
            let content = Rect::from_min_size(pos2((avail - size.x) / 2.0, tops[k]), size);
            if content.max.y < viewport.min.y - size.y || content.min.y > viewport.max.y + size.y {
                continue;
            }
            if top_page.is_none() && content.max.y > viewport.min.y + 40.0 {
                top_page = Some(i);
            }
            let rect = content.translate(origin.to_vec2());
            // Width quantized to 64 px so zooming does not re-render every frame.
            let px = (((size.x * ppp) / 64.0).ceil() * 64.0) as u32;
            v.request_page(i, px);
            let p = ui.painter();
            p.rect_filled(
                rect.translate(vec2(2.0, 3.0)),
                0.0,
                Color32::from_black_alpha(40),
            );
            p.rect_filled(rect, 0.0, Color32::WHITE);
            let tex = v
                .textures
                .get(&(i, px, v.rotate))
                .or_else(|| {
                    v.textures
                        .iter()
                        .find(|((pg, _, r), _)| *pg == i && *r == v.rotate)
                        .map(|(_, t)| t)
                })
                .map(|t| t.id());
            match tex {
                Some(id) => {
                    p.image(id, rect, uv(), Color32::WHITE);
                }
                None => {
                    p.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("Page {}", i + 1),
                        egui::FontId::proportional(14.0),
                        theme::INK_MUTED,
                    );
                }
            }
            for h in v.hits.get(&i).into_iter().flatten() {
                let r = Rect::from_min_max(
                    rect.min + h.min.to_vec2() * rect.size(),
                    rect.min + h.max.to_vec2() * rect.size(),
                )
                .expand(1.5);
                p.rect_filled(r, 2.0, HIT);
            }
            let geom = v.info.as_ref().and_then(|inf| inf.geoms.get(i).copied());
            if let (Some(g), true) = (geom, editable) {
                draw_pending(ui, v, i, rect, s, &g);
                interact(ui, v, i, rect, s, &g, initials);
            }
        }
        // The page indicator follows the scroll (not on the frame a jump was asked for).
        if let (Some(t), Some(d), None) = (top_page, &mut v.doc, jumped)
            && v.continuous
        {
            d.page = t;
        }
    });
    if !editable && v.tool != Tool::Select {
        ui.label(widgets::caption("Rotate the view back to annotate."));
    }
}

/// Screen point of user-space point `p` on page rect `rect`.
fn screen(rect: Rect, s: f32, g: &Geometry, p: [f32; 2]) -> Pos2 {
    let [u, v] = g.to_display(p[0], p[1]);
    rect.min + vec2(u * s, v * s)
}

fn user_rect_to_screen(rect: Rect, s: f32, g: &Geometry, r: [f32; 4]) -> Rect {
    Rect::from_two_pos(
        screen(rect, s, g, [r[0], r[1]]),
        screen(rect, s, g, [r[2], r[3]]),
    )
}

fn color32(c: annot::Rgb) -> Color32 {
    Color32::from_rgb(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
    )
}

fn draw_pending(ui: &egui::Ui, v: &Viewer, page: usize, rect: Rect, s: f32, g: &Geometry) {
    let p = ui.painter();
    for a in v.pending.iter().filter(|a| a.page as usize == page) {
        let c = color32(a.color);
        let stroke = Stroke::new(1.5, c);
        match &a.kind {
            Kind::Highlight(q) => {
                for r in q {
                    p.rect_filled(user_rect_to_screen(rect, s, g, *r), 0.0, PENDING_HL);
                }
            }
            Kind::Underline(q) | Kind::StrikeOut(q) => {
                for r in q {
                    let sr = user_rect_to_screen(rect, s, g, *r);
                    let y = if matches!(a.kind, Kind::Underline(_)) {
                        sr.max.y
                    } else {
                        sr.center().y
                    };
                    p.line_segment([pos2(sr.min.x, y), pos2(sr.max.x, y)], stroke);
                }
            }
            Kind::Note { at, text } => {
                let sr =
                    user_rect_to_screen(rect, s, g, [at[0], at[1] - 20.0, at[0] + 20.0, at[1]]);
                p.rect(
                    sr,
                    2.0,
                    color32(annot::YELLOW),
                    Stroke::new(1.0, Color32::from_rgb(140, 115, 25)),
                    egui::StrokeKind::Inside,
                );
                p.text(
                    sr.right_top() + vec2(4.0, 0.0),
                    egui::Align2::LEFT_TOP,
                    format!("{} · {}", a.author, text),
                    egui::FontId::proportional(11.0),
                    theme::INK,
                );
            }
            Kind::TextBox {
                rect: r,
                text,
                size,
            } => {
                let sr = user_rect_to_screen(rect, s, g, *r);
                p.rect_stroke(sr, 0.0, Stroke::new(1.0, c), egui::StrokeKind::Inside);
                p.text(
                    sr.min + vec2(3.0, 2.0),
                    egui::Align2::LEFT_TOP,
                    text,
                    egui::FontId::proportional(size * s),
                    c,
                );
            }
            Kind::Ink(strokes) => {
                for st in strokes {
                    let pts: Vec<Pos2> = st.iter().map(|q| screen(rect, s, g, *q)).collect();
                    p.add(egui::Shape::line(pts, stroke));
                }
            }
            Kind::Arrow { from, to } => {
                let (a0, b0) = (screen(rect, s, g, *from), screen(rect, s, g, *to));
                p.arrow(a0, b0 - a0, stroke);
            }
            Kind::Tickmark { tick, at, size } => {
                draw_tick(p, screen(rect, s, g, *at), size * s, *tick);
            }
            Kind::Stamp { text, at, size } => {
                let (w, h) = annot::stamp_size(text, *size);
                let c0 = screen(rect, s, g, *at);
                let sr = Rect::from_center_size(c0, vec2(w * s, h * s));
                p.rect_stroke(sr, 0.0, Stroke::new(2.0, c), egui::StrokeKind::Inside);
                p.text(
                    sr.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(size * s),
                    c,
                );
            }
        }
    }
    // Redactions waiting: solid black, red outline, label (UI_SPEC §4).
    let mut redactions: Vec<([f32; 4], String)> = v
        .drawn
        .iter()
        .filter(|r| r.page == page)
        .map(|r| (r.rect, "redact".to_string()))
        .collect();
    for (f, on) in v.pii_found.iter().flatten() {
        if *on && f.page == page {
            redactions.extend(
                f.rects
                    .iter()
                    .map(|r| (*r, format!("{} · redact", f.kind.label()))),
            );
        }
    }
    for (r, label) in redactions {
        let sr = user_rect_to_screen(rect, s, g, r).expand(1.0);
        p.rect(
            sr,
            0.0,
            Color32::BLACK,
            Stroke::new(2.0, Color32::from_rgb(0xC0, 0x1E, 0x1E)),
            egui::StrokeKind::Outside,
        );
        p.text(
            sr.right_top() + vec2(4.0, -2.0),
            egui::Align2::LEFT_BOTTOM,
            label,
            egui::FontId::proportional(10.0),
            Color32::from_rgb(0x9A, 0x1B, 0x1B),
        );
    }
    // The drag in progress.
    if let Some(d) = v.drag.as_ref().filter(|d| d.page == page)
        && let Some(last) = d.points.last()
    {
        let to_s = |q: [f32; 2]| rect.min + vec2(q[0] * s, q[1] * s);
        let st = Stroke::new(1.0, theme::ACCENT_DEFAULT);
        match v.tool {
            Tool::Freehand => {
                let pts: Vec<Pos2> = d.points.iter().map(|q| to_s(*q)).collect();
                p.add(egui::Shape::line(pts, st));
            }
            Tool::Arrow => p.arrow(to_s(d.start), to_s(*last) - to_s(d.start), st),
            _ => {
                p.rect_stroke(
                    Rect::from_two_pos(to_s(d.start), to_s(*last)),
                    0.0,
                    st,
                    egui::StrokeKind::Inside,
                );
            }
        }
    }
}

/// A tickmark drawn the way its appearance stream draws it (shapes, not font glyphs).
fn draw_tick(p: &egui::Painter, c: Pos2, size: f32, t: Tick) {
    let ink = Color32::from_rgb(0x1D, 0x5C, 0x38);
    p.circle(
        c,
        size / 2.0 - 1.0,
        Color32::from_rgb(0xDD, 0xF2, 0xE4),
        Stroke::new(1.0, ink),
    );
    let st = Stroke::new((size / 12.0).max(1.2), ink);
    let at = |x: f32, y: f32| c + vec2((x - 0.5) * size, (0.5 - y) * size);
    match t {
        Tick::Check => {
            p.line_segment([at(0.27, 0.52), at(0.43, 0.34)], st);
            p.line_segment([at(0.43, 0.34), at(0.73, 0.68)], st);
        }
        Tick::Cross => {
            p.line_segment([at(0.32, 0.32), at(0.68, 0.68)], st);
            p.line_segment([at(0.32, 0.68), at(0.68, 0.32)], st);
        }
        Tick::Void => {
            p.circle_stroke(c, size * 0.22, st);
            p.line_segment([at(0.3, 0.3), at(0.7, 0.7)], st);
        }
        _ => {
            let fs = if t.label().len() > 1 {
                size * 0.36
            } else {
                size * 0.55
            };
            p.text(
                c,
                egui::Align2::CENTER_CENTER,
                t.label(),
                egui::FontId::proportional(fs),
                ink,
            );
        }
    }
}

/// Clicks and drags on a page, by tool.
fn interact(
    ui: &mut egui::Ui,
    v: &mut Viewer,
    page: usize,
    rect: Rect,
    s: f32,
    g: &Geometry,
    initials: &str,
) {
    let resp = ui.interact(
        rect,
        ui.scope_id().with(("vf-page", page)),
        Sense::click_and_drag(),
    );
    let to_display = |pos: Pos2| {
        let d = (pos - rect.min) / s;
        [d.x, d.y]
    };
    let to_user = |d: [f32; 2]| g.to_user(d[0], d[1]);
    if matches!(
        v.tool,
        Tool::Select | Tool::Highlight | Tool::Underline | Tool::StrikeOut | Tool::RedactText
    ) {
        v.request_words(page);
    }
    let color = COLORS[v.color].1;
    let author = initials.to_string();
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let at = to_user(to_display(pos));
        match v.tool {
            Tool::Note => {
                v.typing = Some(Typing {
                    page,
                    at: [at[0], at[1], at[0], at[1]],
                    note: true,
                    text: String::new(),
                })
            }
            Tool::Tickmark(tick) => v.pending.push(Annotation {
                page: page as u32,
                kind: Kind::Tickmark {
                    tick,
                    at,
                    size: 28.0,
                },
                color,
                author,
            }),
            _ => {}
        }
        return;
    }
    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let d = to_display(pos);
        v.drag = Some(Drag {
            page,
            start: d,
            points: vec![d],
        });
    }
    if resp.dragged()
        && let (Some(pos), Some(d)) = (resp.interact_pointer_pos(), v.drag.as_mut())
        && d.page == page
    {
        d.points.push(to_display(pos));
    }
    if resp.drag_stopped()
        && let Some(d) = v.drag.take()
        && d.page == page
    {
        let end = *d.points.last().unwrap_or(&d.start);
        let (a, b) = (to_user(d.start), to_user(end));
        let box_user = [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[0].max(b[0]),
            a[1].max(b[1]),
        ];
        let hit_words: Vec<WordBox> = v
            .words
            .get(&page)
            .into_iter()
            .flatten()
            .filter(|w| {
                w.rect[0] < box_user[2]
                    && w.rect[2] > box_user[0]
                    && w.rect[1] < box_user[3]
                    && w.rect[3] > box_user[1]
            })
            .cloned()
            .collect();
        let quads: Vec<[f32; 4]> = hit_words.iter().map(|w| w.rect).collect();
        let push = |v: &mut Viewer, kind| {
            v.pending.push(Annotation {
                page: page as u32,
                kind,
                color,
                author: author.clone(),
            })
        };
        match v.tool {
            Tool::Select => {
                let text: Vec<&str> = hit_words.iter().map(|w| w.text.as_str()).collect();
                if !text.is_empty() {
                    ui.ctx().copy_text(text.join(" "));
                    v.notice = Some(format!("Copied {} words.", text.len()));
                }
            }
            Tool::Highlight if !quads.is_empty() => push(v, Kind::Highlight(quads)),
            Tool::Underline if !quads.is_empty() => push(v, Kind::Underline(quads)),
            Tool::StrikeOut if !quads.is_empty() => push(v, Kind::StrikeOut(quads)),
            Tool::TextBox => {
                v.typing = Some(Typing {
                    page,
                    at: box_user,
                    note: false,
                    text: String::new(),
                })
            }
            Tool::Freehand if d.points.len() > 1 => {
                let stroke = d.points.iter().map(|q| to_user(*q)).collect();
                push(v, Kind::Ink(vec![stroke]))
            }
            Tool::Arrow => push(v, Kind::Arrow { from: a, to: b }),
            Tool::RedactBox if box_user[2] - box_user[0] > 2.0 => v.drawn.push(Redaction {
                page,
                rect: box_user,
            }),
            Tool::RedactText => v
                .drawn
                .extend(quads.iter().map(|r| Redaction { page, rect: *r })),
            _ => {}
        }
    }
}

fn typing_window(ctx: &egui::Context, v: &mut Viewer, initials: &str) {
    let Some(t) = &mut v.typing else { return };
    let mut done = None;
    egui::Window::new(if t.note { "Note" } else { "Text" })
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            let l = ui.label("Text");
            ui.add(egui::TextEdit::multiline(&mut t.text).desired_rows(3))
                .labelled_by(l.id);
            ui.horizontal(|ui| {
                if ui.add(widgets::primary_button("Add")).clicked() {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
        });
    match done {
        Some(true) => {
            let t = v.typing.take().expect("typing");
            if !t.text.trim().is_empty() {
                let kind = if t.note {
                    Kind::Note {
                        at: [t.at[0], t.at[1]],
                        text: t.text,
                    }
                } else {
                    // At least a usable box when only clicked.
                    let r = if t.at[2] - t.at[0] < 20.0 {
                        [t.at[0], t.at[1] - 40.0, t.at[0] + 200.0, t.at[1]]
                    } else {
                        t.at
                    };
                    Kind::TextBox {
                        rect: r,
                        text: t.text,
                        size: 11.0,
                    }
                };
                v.pending.push(Annotation {
                    page: t.page as u32,
                    kind,
                    color: COLORS[v.color].1,
                    author: initials.into(),
                });
            }
        }
        Some(false) => v.typing = None,
        None => {}
    }
}

fn tool_panel(ui: &mut egui::Ui, v: &mut Viewer, env: &mut Env<'_>) {
    egui::Panel::right("vf-viewer-tools")
        .exact_size(320.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(theme::PANE).inner_margin(12))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut v.panel, Panel::Annotate, "Annotate");
                ui.selectable_value(&mut v.panel, Panel::Stamps, "Stamps");
                ui.selectable_value(&mut v.panel, Panel::Redact, "Redact");
            });
            ui.separator();
            match v.panel {
                Panel::Annotate => annotate_panel(ui, v, env),
                Panel::Stamps => stamps_panel(ui, v, env),
                Panel::Redact => redact_panel(ui, v, env),
            }
            if !v.pending.is_empty() {
                ui.separator();
                ui.label(RichText::new(format!("Unsaved ({})", v.pending.len())).strong());
                let mut remove = None;
                for (i, a) in v.pending.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(widgets::caption(format!(
                            "p.{} · {}",
                            a.page + 1,
                            describe(&a.kind)
                        )));
                        if ui.small_button("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    v.pending.remove(i);
                }
                if ui.small_button("Discard all").clicked() {
                    v.pending.clear();
                }
            }
        });
}

fn describe(k: &Kind) -> String {
    match k {
        Kind::Highlight(q) => format!("highlight, {} words", q.len()),
        Kind::Underline(q) => format!("underline, {} words", q.len()),
        Kind::StrikeOut(q) => format!("strikeout, {} words", q.len()),
        Kind::Note { text, .. } => format!("note: {text}"),
        Kind::TextBox { text, .. } => format!("text: {text}"),
        Kind::Ink(_) => "freehand".into(),
        Kind::Arrow { .. } => "arrow".into(),
        Kind::Tickmark { tick, .. } => format!("tickmark: {}", tick.meaning()),
        Kind::Stamp { text, .. } => format!("stamp: {text}"),
    }
}

fn annotate_panel(ui: &mut egui::Ui, v: &mut Viewer, env: &Env<'_>) {
    for (tool, label) in Tool::ANNOTATE {
        ui.radio_value(&mut v.tool, tool, label);
    }
    egui::ComboBox::new("vcolor", "Color")
        .selected_text(COLORS[v.color].0)
        .show_ui(ui, |ui| {
            for (i, (n, _)) in COLORS.iter().enumerate() {
                ui.selectable_value(&mut v.color, i, *n);
            }
        });
    ui.label(widgets::caption(format!("Signed {}", env.initials)));
    ui.add_space(8.0);
    ui.label(RichText::new("Tickmarks").strong());
    ui.horizontal_wrapped(|ui| {
        for t in Tick::ALL {
            let on = v.tool == Tool::Tickmark(t);
            if ui
                .selectable_label(on, tick_name(t))
                .on_hover_text(format!("Tickmark: {}", t.meaning()))
                .clicked()
            {
                v.tool = Tool::Tickmark(t);
            }
        }
    });
    ui.label(widgets::caption(
        "Stamped as real PDF annotations with author and date.",
    ));
}

const DANGER: Color32 = Color32::from_rgb(0x9A, 0x1B, 0x1B);

/// The Redact tab: modes, auto-find results with checkboxes, Apply & verify.
fn redact_panel(ui: &mut egui::Ui, v: &mut Viewer, env: &mut Env<'_>) {
    ui.label(RichText::new("Mode").strong());
    ui.radio_value(&mut v.tool, Tool::RedactBox, "Draw box");
    ui.radio_value(&mut v.tool, Tool::RedactText, "Select text");
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("Auto-find patterns").clicked()
            && let Some(r) = &v.renderer
        {
            r.send(Job::Pii {
                generation: v.generation,
                kinds: v.pii_kinds.clone(),
            });
        }
        for (k, label) in [
            (Pii::Account, "+ Account numbers"),
            (Pii::Dob, "+ Dates of birth"),
        ] {
            let mut on = v.pii_kinds.contains(&k);
            if ui.toggle_value(&mut on, label).changed() {
                v.pii_kinds.retain(|x| *x != k);
                if on {
                    v.pii_kinds.push(k);
                }
                v.pii_found = None;
            }
        }
    });
    if let Some(found) = &mut v.pii_found {
        ui.label(RichText::new(format!("Auto-find results ({})", found.len())).strong());
        if found.is_empty() {
            ui.label(widgets::caption("No SSN, EIN or other pattern found."));
        }
        egui::ScrollArea::vertical()
            .id_salt("vfound")
            .max_height(220.0)
            .show(ui, |ui| {
                for (f, on) in found.iter_mut() {
                    let text = format!("{} · page {}  {}", f.kind.label(), f.page + 1, f.masked());
                    let r = ui.checkbox(on, text);
                    if f.kind == Pii::Ein {
                        r.on_hover_text("(payer — usually keep)");
                    }
                }
            });
    }
    let n = v.drawn.len() + v.pii_found.iter().flatten().filter(|(_, on)| *on).count();
    ui.add_space(6.0);
    egui::Frame::new()
        .fill(theme::APP_BG)
        .inner_margin(8)
        .corner_radius(4)
        .show(ui, |ui| {
            ui.label(widgets::caption(
                "Apply removes the text and image content under the boxes, strips the metadata and saves \"… (redacted).pdf\" beside this document, which is not changed. It then re-reads the result and refuses to save if anything removed is still there. Counts go to the audit trail.",
            ));
        });
    let apply = ui.add_enabled(
        n > 0,
        egui::Button::new(
            RichText::new(format!(
                "Apply {n} {} & verify",
                if n == 1 { "redaction" } else { "redactions" }
            ))
            .color(Color32::WHITE)
            .strong(),
        )
        .fill(DANGER)
        .min_size(vec2(ui.available_width(), 40.0)),
    );
    if !v.drawn.is_empty() && ui.small_button("Clear drawn boxes").clicked() {
        v.drawn.clear();
    }
    if apply.clicked() {
        apply_redactions(v, env);
    }
}

fn apply_redactions(v: &mut Viewer, env: &mut Env<'_>) {
    let Some(doc) = v.doc.clone() else { return };
    let plan = crate::redact::Plan {
        found: v
            .pii_found
            .iter()
            .flatten()
            .filter(|(_, on)| *on)
            .map(|(f, _)| f.clone())
            .collect(),
        drawn: v.drawn.clone(),
    };
    let pdfium = crate::ui::intake::pdfium_dir()
        .ok_or_else(|| "pdfium.dll not found".to_string())
        .and_then(|d| filer_pdf::pdf::pdfium(&d));
    let r = pdfium.and_then(|p| {
        crate::redact::redact_document(env.conn, env.root, env.device_id, p, &doc.doc_id, &plan)
    });
    match r {
        Ok(added) => {
            v.catalog_changed = true;
            let counts = plan
                .counts()
                .iter()
                .map(|(k, n)| format!("{n} {k}"))
                .collect::<Vec<_>>()
                .join(", ");
            let d = Doc::at(env.root, &added.doc_id, &added.rel_path, doc.page, vec![]);
            let name = d.name.clone();
            v.open(d);
            v.notice = Some(format!(
                "Redacted and verified ({counts}): saved as {name}. The original is not changed."
            ));
        }
        Err(e) => v.error = Some(e),
    }
}

/// Palette label without symbol glyphs (the bundled fonts lack ✓ ✗ ⊘ ƒ).
fn tick_name(t: Tick) -> &'static str {
    match t {
        Tick::Check => "Check",
        Tick::Cross => "Cross",
        Tick::Void => "Void",
        Tick::Footed => "f",
        Tick::TiedToBooks => "T/B",
        Tick::PriorYear => "PY",
        Tick::CurrentYear => "CY",
    }
}

fn stamps_panel(ui: &mut egui::Ui, v: &mut Viewer, env: &Env<'_>) {
    let today = vf_core::now_rfc3339();
    for p in Preset::ALL {
        ui.radio_value(&mut v.preset, p, p.label());
    }
    egui::ComboBox::new("vplacement", "Placement")
        .selected_text(v.placement.label())
        .show_ui(ui, |ui| {
            for p in Placement::ALL {
                ui.selectable_value(&mut v.placement, p, p.label());
            }
        });
    let text = v.preset.text(&today, env.initials);
    ui.label(widgets::caption(format!(
        "Stamps \"{text}\" signed {}",
        env.initials
    )));
    let (this, all) = ui
        .horizontal(|ui| {
            (
                ui.button("Stamp this page").clicked(),
                ui.button("Stamp all pages").clicked(),
            )
        })
        .inner;
    if (this || all)
        && let (Some(info), Some(doc)) = (&v.info, &v.doc)
    {
        let pages: Vec<usize> = if all {
            (0..info.geoms.len()).collect()
        } else {
            vec![doc.page]
        };
        let stamps: Vec<Annotation> = pages
            .into_iter()
            .map(|p| Annotation {
                page: p as u32,
                kind: Kind::Stamp {
                    text: text.clone(),
                    at: annot::placed(&info.geoms[p], &text, 16.0, v.placement),
                    size: 16.0,
                },
                color: annot::ACCENT,
                author: env.initials.into(),
            })
            .collect();
        v.pending.extend(stamps);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Convert {
    Flatten,
    PdfA,
    Compress,
}

/// Flatten or PDF/A: the result is a new version of this document (current one kept).
fn convert(v: &mut Viewer, env: &mut Env<'_>, what: Convert) {
    let Some(doc) = v.doc.clone() else { return };
    let r = (|| -> Result<String, String> {
        let tmp = vf_fs::content::tmp_file(env.root, "pdf")?;
        let (label, reason, detail) = match what {
            Convert::Flatten => {
                let n = vf_pdf::flatten::flatten(&doc.path, &tmp).inspect_err(|_| {
                    let _ = std::fs::remove_file(&tmp);
                })?;
                (
                    "flatten",
                    "before flatten",
                    serde_json::json!({ "annotations": n }),
                )
            }
            Convert::Compress => {
                let r =
                    vf_pdf::compress::compress(&doc.path, &tmp, 200.0, 70).inspect_err(|_| {
                        let _ = std::fs::remove_file(&tmp);
                    })?;
                (
                    "compress",
                    "before compress",
                    serde_json::json!({ "images": r.images, "before": r.before, "after": r.after }),
                )
            }
            Convert::PdfA => {
                let pdfium = crate::ui::intake::pdfium_dir()
                    .ok_or_else(|| "pdfium.dll not found".to_string())
                    .and_then(|d| filer_pdf::pdf::pdfium(&d))?;
                let title = doc.name.trim_end_matches(".pdf").to_string();
                let rep =
                    vf_pdf::pdfa::convert(pdfium, &doc.path, &tmp, &title, &vf_core::now_rfc3339())
                        .inspect_err(|_| {
                            let _ = std::fs::remove_file(&tmp);
                        })?;
                (
                    "pdfa",
                    "before PDF/A",
                    serde_json::json!({
                        "rasterized_pages": rep.rasterized.len(),
                        "flattened": rep.flattened,
                    }),
                )
            }
        };
        if what == Convert::Compress && detail["after"].as_u64() >= detail["before"].as_u64() {
            let _ = std::fs::remove_file(&tmp);
            return Ok("Already compact: nothing to gain, the file is not changed.".into());
        }
        let change = vf_fs::content::Change {
            label,
            created_by: label,
            reason,
            detail: detail.clone(),
            extra: vec![],
        };
        match vf_fs::content::replace(env.conn, env.root, env.device_id, &doc.doc_id, &tmp, change)?
        {
            Ok(_) => Ok(match what {
                Convert::Flatten => format!(
                    "Flattened {} annotations into the pages; the previous version is kept.",
                    detail["annotations"]
                ),
                Convert::PdfA => format!(
                    "Saved as PDF/A-2b ({} pages rebuilt as images with a text layer); the previous version is kept.",
                    detail["rasterized_pages"]
                ),
                Convert::Compress => format!(
                    "Compressed {} images: {} → {}; the previous version is kept.",
                    detail["images"],
                    widgets::size(detail["before"].as_i64().unwrap_or(0)),
                    widgets::size(detail["after"].as_i64().unwrap_or(0))
                ),
            }),
            Err(why) => Err(format!("Not changed: {why}.")),
        }
    })();
    match r {
        Ok(msg) => {
            v.catalog_changed = true;
            v.reload();
            v.notice = Some(msg);
        }
        Err(e) => v.error = Some(e),
    }
}

/// Writes the pending annotations: a new version of this document, or (`as_new`) a new
/// document beside it.
fn save(v: &mut Viewer, env: &mut Env<'_>, as_new: bool) {
    let Some(doc) = v.doc.clone() else { return };
    let n = v.pending.len();
    let r = save_pending(v, env, &doc, as_new);
    match r {
        Ok(new_doc) => {
            v.pending.clear();
            v.catalog_changed = true;
            match new_doc {
                Some(d) => {
                    let name = d.name.clone();
                    v.open(d);
                    v.notice = Some(format!("Saved as a new document: {name}."));
                }
                None => {
                    v.reload();
                    v.notice = Some(format!(
                        "Saved {n} annotations as a new version; the previous one is kept."
                    ));
                }
            }
        }
        Err(e) => v.error = Some(e),
    }
}

fn save_pending(
    v: &Viewer,
    env: &mut Env<'_>,
    doc: &Doc,
    as_new: bool,
) -> Result<Option<Doc>, String> {
    let n = v.pending.len();
    let tmp = vf_fs::content::tmp_file(env.root, "pdf")?;
    let now = vf_core::now_rfc3339();
    if let Err(e) = annot::add(&doc.path, &tmp, &v.pending, &now) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if as_new {
        let row: (
            String,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
        ) = env
            .conn
            .query_row(
                "SELECT rel_path, client_id, doc_type_id, tax_year, source FROM document
                 WHERE id = ?1",
                [&doc.doc_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .map_err(|e| e.to_string())?;
        let (rel, client_id, doc_type_id, tax_year, source) = row;
        let added = vf_fs::content::add(
            env.conn,
            env.root,
            env.device_id,
            &tmp,
            vf_fs::content::NewDoc {
                rel_path: rel,
                client_id,
                doc_type_id,
                tax_year,
                source,
                tags: vec![format!("copy-of:{}", doc.doc_id)],
                label: "save_as".into(),
                detail: serde_json::json!({ "from": doc.doc_id, "annotations": n }),
            },
        )?;
        return Ok(Some(Doc::at(
            env.root,
            &added.doc_id,
            &added.rel_path,
            doc.page,
            vec![],
        )));
    }
    let change = vf_fs::content::Change {
        label: "annotate",
        created_by: "annotate",
        reason: "before annotations",
        detail: serde_json::json!({ "annotations": n }),
        extra: vec![],
    };
    match vf_fs::content::replace(env.conn, env.root, env.device_id, &doc.doc_id, &tmp, change)? {
        Ok(_) => Ok(None),
        Err(why) => Err(format!("Not saved: {why}.")),
    }
}
