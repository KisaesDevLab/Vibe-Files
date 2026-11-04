//! The Search tab (P03 step 5): one box, filters, results with a thumbnail of the matching
//! page and a snippet; a result opens in the Viewer at that page.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use egui::{Color32, RichText};
use vf_core::browse::Snapshot;
use vf_core::db::Connection;
use vf_index::search::{self, DocType, Hit, Query, Scope, Year};

use crate::ui::pages::{Renderer, Request};
use crate::ui::viewer::Doc;
use crate::ui::{theme, widgets};

const LIMIT: usize = 200;
const THUMB_WIDTH: u32 = 96;
const ROW_HEIGHT: f32 = 132.0;
const MARK_BG: Color32 = Color32::from_rgb(0xFF, 0xE8, 0x8A);

/// What "This client" and "This folder" mean: the node selected in Browse.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Context {
    /// (client id, label).
    pub client: Option<(String, String)>,
    /// (cabinet-relative folder, its year, label).
    pub folder: Option<(String, Option<i32>, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Where {
    #[default]
    Cabinet,
    Client,
    Folder,
}

#[derive(Default)]
pub struct SearchTab {
    pub text: String,
    pub scope: Where,
    /// Filters (the query's metadata fields; text and scope are filled in when it runs).
    pub filters: Query,
    year_text: String,
    pub results: Vec<Hit>,
    pub elapsed: Option<Duration>,
    pub error: Option<String>,
    /// The query the results are for.
    ran: Option<Query>,
    renderer: Option<Renderer>,
    thumbs: HashMap<String, egui::TextureHandle>,
    requested: HashSet<String>,
    /// A result to open in the Viewer (taken by the app).
    pub open: Option<Doc>,
    /// The "Save search" name being typed.
    save_name: Option<String>,
    /// A search was saved: the app reloads Browse's Saved searches.
    pub saved_changed: bool,
    pub notice: Option<String>,
    near_job: Option<std::sync::mpsc::Receiver<Result<Vec<vf_index::neardup::Pair>, String>>>,
    /// The last near-duplicate report.
    pub near: Option<Result<Vec<vf_index::neardup::Pair>, String>>,
}

impl SearchTab {
    /// The query as the box, scope and filters say.
    pub fn query(&self, ctx: &Context) -> Query {
        let mut q = self.filters.clone();
        q.text = self.text.trim().to_string();
        q.scope = Scope::Cabinet;
        match self.scope {
            Where::Cabinet => {}
            Where::Client => {
                if let Some((id, _)) = &ctx.client {
                    q.scope = Scope::Client(id.clone());
                }
            }
            Where::Folder => {
                if let Some((f, year, _)) = &ctx.folder {
                    q.scope = Scope::Folder(f.clone());
                    if q.year.is_none() {
                        q.year = Some(year.map_or(Year::Permanent, Year::Year));
                    }
                }
            }
        }
        q
    }

    /// Runs the query when it changed since the last run.
    pub fn run_if_changed(&mut self, conn: &Connection, ctx: &Context) {
        let q = self.query(ctx);
        if self.ran.as_ref() == Some(&q) {
            return;
        }
        self.ran = Some(q.clone());
        if q.is_empty() {
            self.results.clear();
            self.elapsed = None;
            return;
        }
        let t = Instant::now();
        match search::search(conn, &q, LIMIT) {
            Ok(r) => {
                self.results = r;
                self.error = None;
            }
            Err(e) => {
                self.results.clear();
                self.error = Some(format!("Search failed: {e}"));
            }
        }
        self.elapsed = Some(t.elapsed());
    }

    /// Searches again on the next frame (the catalog changed).
    pub fn refresh(&mut self) {
        self.ran = None;
    }

    fn terms(&self) -> Vec<String> {
        self.text.split_whitespace().map(str::to_string).collect()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    s: &mut SearchTab,
    conn: &Connection,
    catalog: &Path,
    root: &Path,
    snap: &Snapshot,
    ctx: &Context,
    device_id: &str,
) {
    pump_thumbs(ui.ctx(), s);
    egui::Panel::top("vf-search-box")
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(theme::pane())
                .inner_margin(egui::Margin::symmetric(16, 10)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let label = ui.label("Search for");
                ui.add(
                    egui::TextEdit::singleline(&mut s.text)
                        .hint_text("text, names, EINs…")
                        .desired_width(420.0),
                )
                .labelled_by(label.id);
                ui.separator();
                ui.radio_value(&mut s.scope, Where::Cabinet, "Cabinet");
                let client = ctx.client.as_ref().map(|c| c.1.as_str());
                ui.add_enabled_ui(client.is_some(), |ui| {
                    ui.radio_value(&mut s.scope, Where::Client, "This client")
                        .on_hover_text(client.unwrap_or("Select a client in Browse"));
                });
                let folder = ctx.folder.as_ref().map(|f| f.2.as_str());
                ui.add_enabled_ui(folder.is_some(), |ui| {
                    ui.radio_value(&mut s.scope, Where::Folder, "This folder")
                        .on_hover_text(folder.unwrap_or("Select a folder in Browse"));
                });
            });
            filters(ui, s, snap);
            save_row(ui, s, root, ctx, device_id);
            near_row(ui, s, catalog, root);
        });
    s.run_if_changed(conn, ctx);
    egui::CentralPanel::default().show(ui, |ui| {
        widgets::message_line(ui, s.error.as_deref());
        if s.ran.as_ref().is_none_or(Query::is_empty) {
            ui.label(widgets::caption(
                "Type to search the text of every document (scans too, once OCR has read them), file names and metadata.",
            ));
            return;
        }
        ui.label(widgets::caption(format!(
            "{} documents{}{}",
            s.results.len(),
            if s.results.len() == LIMIT { " (first 200)" } else { "" },
            s.elapsed
                .map(|d| format!(" · {} ms", d.as_millis()))
                .unwrap_or_default()
        )));
        let terms = s.terms();
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            ROW_HEIGHT,
            s.results.len(),
            |ui, range| {
                for i in range {
                    let hit = s.results[i].clone();
                    let key = thumb_key(&hit);
                    if !s.requested.contains(&key)
                        && let Some(r) = &s.renderer
                    {
                        s.requested.insert(key.clone());
                        r.request(Request {
                            key: key.clone(),
                            path: root.join(&hit.rel_path),
                            page: hit.page.unwrap_or(0) as usize,
                            width: THUMB_WIDTH,
                            terms: vec![],
                        });
                    }
                    if row(ui, &hit, s.thumbs.get(&key)).clicked() {
                        s.open = Some(Doc::at(
                            root,
                            &hit.doc_id,
                            &hit.rel_path,
                            hit.page.unwrap_or(0) as usize,
                            terms.clone(),
                        ));
                    }
                }
            },
        );
    });
}

/// `Save search…`: names the current query; Browse lists it under Saved searches for the
/// whole firm.
fn save_row(ui: &mut egui::Ui, s: &mut SearchTab, root: &Path, ctx: &Context, device_id: &str) {
    let q = s.query(ctx);
    ui.horizontal(|ui| {
        match &mut s.save_name {
            None => {
                if ui
                    .add_enabled(!q.is_empty(), egui::Button::new("Save search…"))
                    .on_hover_text("Saved searches appear in Browse for everyone in the firm.")
                    .clicked()
                {
                    s.save_name = Some(String::new());
                    s.notice = None;
                }
            }
            Some(name) => {
                let l = ui.label("Name");
                ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text("e.g. 1099s received 2025")
                        .desired_width(220.0),
                )
                .labelled_by(l.id);
                if ui.add(widgets::primary_button("Save")).clicked() {
                    match vf_index::saved::add(root, name, &q, device_id) {
                        Ok(_) => {
                            s.notice = Some(format!(
                                "Saved \"{}\". It is in Browse under Saved searches.",
                                name.trim()
                            ));
                            s.saved_changed = true;
                            s.save_name = None;
                        }
                        Err(e) => s.notice = Some(e),
                    }
                } else if ui.button("Cancel").clicked() {
                    s.save_name = None;
                }
            }
        }
        if let Some(n) = &s.notice {
            ui.label(widgets::caption(n));
        }
    });
}

/// Similarity at or above which two documents are reported as near duplicates.
const NEAR: f32 = 0.8;

/// `Near-duplicate report`: documents whose text is almost the same but whose files differ
/// (a rescan, a printed and a scanned copy). Runs on its own thread and catalog connection.
fn near_row(ui: &mut egui::Ui, s: &mut SearchTab, catalog: &Path, root: &Path) {
    if let Some(rx) = &s.near_job
        && let Ok(r) = rx.try_recv()
    {
        s.near = Some(r);
        s.near_job = None;
    }
    ui.horizontal(|ui| {
        let busy = s.near_job.is_some();
        if ui
            .add_enabled(!busy, egui::Button::new("Near-duplicate report"))
            .on_hover_text("Documents whose text is at least 80 % the same but whose files differ.")
            .clicked()
        {
            let catalog = catalog.to_path_buf();
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let r = vf_core::db::open(&catalog, vf_core::db::Storage::Local)
                    .and_then(|c| vf_index::neardup::report(&c, NEAR))
                    .map_err(|e| e.to_string());
                let _ = tx.send(r);
                ctx.request_repaint();
            });
            s.near_job = Some(rx);
        }
        if busy {
            ui.spinner();
        }
        match &s.near {
            Some(Ok(p)) if p.is_empty() => {
                ui.label(widgets::caption("No near duplicates."));
            }
            Some(Ok(p)) => {
                ui.label(widgets::caption(format!("{} pairs", p.len())));
            }
            Some(Err(e)) => {
                ui.label(widgets::caption(format!("Report failed: {e}")));
            }
            None => {}
        }
        if s.near.is_some() && ui.small_button("Close report").clicked() {
            s.near = None;
        }
    });
    let mut open = None;
    if let Some(Ok(pairs)) = &s.near {
        egui::ScrollArea::vertical()
            .id_salt("near-dups")
            .max_height(180.0)
            .show(ui, |ui| {
                for p in pairs {
                    ui.horizontal(|ui| {
                        ui.label(format!("{:.0} %", p.similarity * 100.0));
                        for (id, path) in [(&p.a, &p.a_path), (&p.b, &p.b_path)] {
                            if ui.link(path).clicked() {
                                open = Some(Doc::at(root, id, path, 0, vec![]));
                            }
                        }
                    });
                }
            });
    }
    if open.is_some() {
        s.open = open;
    }
}

fn filters(ui: &mut egui::Ui, s: &mut SearchTab, snap: &Snapshot) {
    let f = &mut s.filters;
    ui.horizontal_wrapped(|ui| {
        let client_name = f
            .client
            .as_ref()
            .and_then(|id| snap.clients.iter().find(|c| &c.id == id))
            .map_or("Any client".to_string(), |c| {
                format!("{} ({})", c.name, c.ext_id)
            });
        ui.horizontal(|ui| {
            let l = ui.label("Client");
            egui::ComboBox::from_id_salt("search-client")
                .selected_text(client_name)
                .width(180.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.client, None, "Any client");
                    for c in &snap.clients {
                        ui.selectable_value(
                            &mut f.client,
                            Some(c.id.clone()),
                            format!("{} ({})", c.name, c.ext_id),
                        );
                    }
                })
                .response
                .labelled_by(l.id);
        });
        ui.horizontal(|ui| {
            let yl = ui.label("Year");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut s.year_text)
                        .hint_text("any")
                        .desired_width(70.0),
                )
                .labelled_by(yl.id)
                .on_hover_text("2025, Permanent, or empty for any year")
                .changed()
            {
                let t = s.year_text.trim();
                f.year = if t.eq_ignore_ascii_case("permanent") || t.eq_ignore_ascii_case("p") {
                    Some(Year::Permanent)
                } else {
                    t.parse().ok().map(Year::Year)
                };
            }
        });
        let dt_name = match &f.doc_type {
            None => "Any doc type".to_string(),
            Some(DocType::Needed) => "Needs doc type".to_string(),
            Some(DocType::Is(id)) => snap
                .doc_types
                .iter()
                .find(|(i, _)| i == id)
                .map_or("?".into(), |(_, n)| n.clone()),
        };
        ui.horizontal(|ui| {
            let l = ui.label("Doc type");
            egui::ComboBox::from_id_salt("search-doctype")
                .selected_text(dt_name)
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.doc_type, None, "Any doc type");
                    ui.selectable_value(&mut f.doc_type, Some(DocType::Needed), "Needs doc type");
                    for (id, name) in &snap.doc_types {
                        ui.selectable_value(&mut f.doc_type, Some(DocType::Is(id.clone())), name);
                    }
                })
                .response
                .labelled_by(l.id);
        });
        combo(
            ui,
            "search-status",
            "Status",
            "Any status",
            &mut f.status,
            &vf_fs::docmeta::STATUSES,
        );
        combo(
            ui,
            "search-source",
            "Source",
            "Any source",
            &mut f.source,
            &vf_fs::docmeta::SOURCES,
        );
        text_filter(ui, "Filed from", &mut f.filed_from, "YYYY-MM-DD");
        text_filter(ui, "to", &mut f.filed_to, "YYYY-MM-DD");
        text_filter(ui, "Filed by", &mut f.filed_by, "anyone");
        text_filter(ui, "Tag", &mut f.tag, "any");
        ui.checkbox(&mut f.has_ssn, "Has SSN");
        if *f != Query::default() && ui.small_button("Clear filters").clicked() {
            *f = Query::default();
            s.year_text.clear();
        }
    });
}

fn combo(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    any: &'static str,
    v: &mut Option<String>,
    opts: &[(&str, &'static str)],
) {
    ui.horizontal(|ui| {
        let l = ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(widgets::label_of(opts, v.as_deref(), any))
            .show_ui(ui, |ui| {
                ui.selectable_value(v, None, any);
                for (key, name) in opts {
                    ui.selectable_value(v, Some((*key).to_string()), *name);
                }
            })
            .response
            .labelled_by(l.id);
    });
}

fn text_filter(ui: &mut egui::Ui, label: &str, v: &mut Option<String>, hint: &str) {
    ui.horizontal(|ui| {
        let l = ui.label(label);
        let mut t = v.clone().unwrap_or_default();
        if ui
            .add(
                egui::TextEdit::singleline(&mut t)
                    .hint_text(hint)
                    .desired_width(100.0),
            )
            .labelled_by(l.id)
            .changed()
        {
            *v = (!t.trim().is_empty()).then(|| t.trim().to_string());
        }
    });
}

fn thumb_key(h: &Hit) -> String {
    format!("{}#{}", h.doc_id, h.page.unwrap_or(0))
}

fn pump_thumbs(ctx: &egui::Context, s: &mut SearchTab) {
    let r = s
        .renderer
        .get_or_insert_with(|| Renderer::new(ctx, crate::ui::intake::pdfium_dir(), false));
    for (key, res) in r.finished() {
        if let Ok(p) = res {
            let tex = ctx.load_texture(
                format!("search-{key}"),
                p.image,
                egui::TextureOptions::LINEAR,
            );
            s.thumbs.insert(key, tex);
        }
    }
    if s.thumbs.len() > 400 {
        s.thumbs.clear();
        s.requested.clear();
    }
}

/// One result: thumbnail, name, metadata, snippet. The whole row clicks.
fn row(ui: &mut egui::Ui, h: &Hit, thumb: Option<&egui::TextureHandle>) -> egui::Response {
    let inner = ui.horizontal(|ui| {
        ui.set_min_height(ROW_HEIGHT - 8.0);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(THUMB_WIDTH as f32, 124.0), egui::Sense::hover());
        match thumb {
            Some(t) => {
                let size = t.size_vec2() * (THUMB_WIDTH as f32 / t.size_vec2().x);
                let r = egui::Rect::from_min_size(rect.min, size.min(rect.size()));
                ui.painter().image(
                    t.id(),
                    r,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                ui.painter().rect_stroke(
                    r,
                    0.0,
                    (1.0, theme::border_pane()),
                    egui::StrokeKind::Outside,
                );
            }
            None => {
                ui.painter().rect_filled(rect, 2.0, theme::app_bg());
            }
        }
        ui.vertical(|ui| {
            ui.add(
                egui::Label::new(RichText::new(widgets::file_name(&h.rel_path)).strong())
                    .selectable(false),
            );
            let mut meta = vec![
                h.client.clone().unwrap_or_else(|| "No client".into()),
                widgets::year_label(h.tax_year),
                h.doc_type
                    .clone()
                    .unwrap_or_else(|| "Needs doc type".into()),
            ];
            match h.page {
                Some(p) if h.pages > 1 => {
                    meta.push(format!("page {} ({} pages match)", p + 1, h.pages))
                }
                Some(p) => meta.push(format!("page {}", p + 1)),
                None => meta.push("name or details match".into()),
            }
            ui.add(egui::Label::new(widgets::caption(meta.join(" · "))).selectable(false));
            ui.add(egui::Label::new(widgets::caption(&h.rel_path).small()).selectable(false));
            if !h.snippet.is_empty() {
                let mut job = egui::text::LayoutJob::default();
                for (text, on) in search::runs(&h.snippet) {
                    let mut fmt = egui::TextFormat {
                        color: theme::ink(),
                        ..Default::default()
                    };
                    if on {
                        fmt.background = MARK_BG;
                    }
                    job.append(&text, 0.0, fmt);
                }
                job.wrap.max_width = ui.available_width();
                ui.add(egui::Label::new(job).selectable(false));
            }
        });
    });
    let resp = inner.response.interact(egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_stroke(
            resp.rect,
            4.0,
            (1.0, theme::accent()),
            egui::StrokeKind::Inside,
        );
    }
    ui.separator();
    resp.on_hover_text("Open in the Viewer at this page")
}
