//! Admin › Bulk changes (owner 2026-11-15, Q-117, PA-78): find the documents whose file name
//! contains a phrase, tick some or all of them, and change them together: doc type, tax year,
//! status, source and tags; a find-and-replace in their names; or the trash.
//!
//! The find reads every document's path and each change touches many files, so both run on a
//! thread with their own catalog connection and a spinner. Each change is one Transaction
//! (audited, undoable from Browse), made by the same code Browse uses for a selection.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use egui::{Align, Layout, RichText};
use egui_extras::{Column, TableBuilder};
use vf_core::browse::DocRow;
use vf_core::db::Connection;
use vf_fs::rename::{Outcome, Row};

use crate::ui::browse::Env;
use crate::ui::job::{self, Job};
use crate::ui::meta_editor::{self, MetaForm};
use crate::ui::theme;
use crate::ui::widgets::{self, UiExt as _};

/// The most documents one find lists (as Browse's Find documents…).
pub const LIMIT: usize = 10_000;
const ROW_H: f32 = 24.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Col {
    #[default]
    Name,
    Client,
    Folder,
    DocType,
    Year,
    Status,
    Source,
    Size,
}

const COLS: [(Col, &str); 8] = [
    (Col::Name, "Name"),
    (Col::Client, "Client"),
    (Col::Folder, "Folder"),
    (Col::DocType, "Doc type"),
    (Col::Year, "Year"),
    (Col::Status, "Status"),
    (Col::Source, "Source"),
    (Col::Size, "Size"),
];

/// The documents one transaction of a change takes. A larger change is made in parts of this
/// many, so its count can be shown and it can be stopped between two parts.
const PART: usize = 500;

/// How far a change has come, and whether it was asked to stop.
#[derive(Default)]
struct Progress {
    done: std::sync::atomic::AtomicUsize,
    of: std::sync::atomic::AtomicUsize,
    stop: std::sync::atomic::AtomicBool,
}

/// How a change in parts ended.
struct Parts {
    /// Documents changed.
    changed: usize,
    /// Transactions made.
    parts: usize,
    /// Asked to stop before the end.
    stopped: bool,
}

/// `each` over `items` in parts of [`PART`], counting in `pr` and stopping between two parts
/// when asked. A part that fails stops it, saying how many were done before.
fn in_parts<T>(
    items: &[T],
    pr: &Progress,
    mut each: impl FnMut(&[T]) -> Result<usize, String>,
) -> Result<Parts, String> {
    use std::sync::atomic::Ordering::Relaxed;
    pr.of.store(items.len(), Relaxed);
    let mut out = Parts {
        changed: 0,
        parts: 0,
        stopped: false,
    };
    for (i, part) in items.chunks(PART).enumerate() {
        if pr.stop.load(Relaxed) {
            out.stopped = true;
            break;
        }
        match each(part) {
            Ok(0) => {}
            Ok(n) => {
                out.changed += n;
                out.parts += 1;
            }
            Err(e) if out.changed > 0 => {
                return Err(format!(
                    "{e} ({} were changed before this; Undo last in Browse takes them back a part at a time.)",
                    docs(out.changed)
                ));
            }
            Err(e) => return Err(e),
        }
        pr.done
            .store((i * PART + part.len()).min(items.len()), Relaxed);
    }
    Ok(out)
}

/// The note after a change: what was done to how many, and how Undo takes it back.
fn said(done: &str, p: &Parts, of: usize, back: &str) -> String {
    let what = if p.stopped {
        format!("Stopped: {done} {} of {}.", count(p.changed), docs(of))
    } else {
        format!("{} {}.", capital(done), docs(p.changed))
    };
    match p.parts {
        0 => what,
        1 => format!("{what} {back}"),
        n => format!(
            "{what} Done in {n} transactions of up to {PART}: Undo last in Browse takes them back one at a time."
        ),
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or(String::new(), |f| {
        f.to_uppercase().collect::<String>() + c.as_str()
    })
}

/// What a find gives back.
struct Found {
    phrase: String,
    rows: Vec<DocRow>,
    more: bool,
    clients: HashMap<String, String>,
    doc_types: Vec<(String, String)>,
}

/// The find-and-replace in the ticked documents' names.
#[derive(Default)]
struct Renaming {
    find: String,
    replace: String,
    /// The preview, for the texts in `planned_for` and the documents ticked then.
    plan: Option<Vec<Row>>,
    planned_for: (String, String),
    planned_ids: BTreeSet<String>,
    job: Option<Job<Result<Vec<Row>, String>>>,
    error: Option<String>,
}

#[derive(Default)]
pub struct BulkPage {
    pub phrase: String,
    /// The phrase the rows were found for.
    found_for: Option<String>,
    rows: Vec<DocRow>,
    more: bool,
    clients: HashMap<String, String>,
    doc_types: Vec<(String, String)>,
    /// Ticked document ids.
    pub picked: BTreeSet<String>,
    form: MetaForm,
    form_for: BTreeSet<String>,
    sort: (Col, bool),
    finding: Option<Job<Result<Found, String>>>,
    /// A change running; its result is the note to show.
    work: Option<Job<Result<String, String>>>,
    progress: std::sync::Arc<Progress>,
    confirm_trash: bool,
    renaming: Option<Renaming>,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The catalog changed: Browse re-reads.
    pub changed: bool,
    /// A change finished: the grid is found again when the page is next drawn.
    refind: bool,
}

impl BulkPage {
    /// Sets the tax year in the details form (tests).
    #[cfg(test)]
    pub fn set_year_for_test(&mut self, year: Option<i32>) {
        self.form.edit.tax_year = Some(year);
    }

    #[cfg(test)]
    pub fn rows(&self) -> &[DocRow] {
        &self.rows
    }

    fn find(&mut self, ctx: &egui::Context, catalog: &Path) {
        let phrase = self.phrase.trim().to_string();
        if phrase.is_empty() {
            self.error = Some("Enter part of a file name to find.".into());
            return;
        }
        let catalog = catalog.to_path_buf();
        match Job::start(ctx, "Finding documents…", move || {
            let conn = vf_core::db::open(&catalog, vf_core::db::Storage::Local)
                .map_err(|e| e.to_string())?;
            let mut rows = vf_core::browse::docs_named(&conn, &phrase, LIMIT + 1)
                .map_err(|e| e.to_string())?;
            let more = rows.len() > LIMIT;
            rows.truncate(LIMIT);
            let pairs = |sql: &str| -> Result<Vec<(String, String)>, String> {
                let mut st = conn.prepare(sql).map_err(|e| e.to_string())?;
                st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .and_then(|rows| rows.collect())
                    .map_err(|e| e.to_string())
            };
            Ok(Found {
                phrase,
                rows,
                more,
                clients: pairs("SELECT id, name FROM client")?.into_iter().collect(),
                doc_types: pairs(
                    "SELECT id, name FROM doc_type WHERE deleted_at IS NULL AND archived = 0
                     ORDER BY sort, name",
                )?,
            })
        }) {
            Ok(j) => {
                self.finding = Some(j);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn found(&mut self, f: Found) {
        let live: BTreeSet<&str> = f.rows.iter().map(|d| d.id.as_str()).collect();
        self.picked.retain(|id| live.contains(id.as_str()));
        self.found_for = Some(f.phrase);
        self.rows = f.rows;
        self.more = f.more;
        self.clients = f.clients;
        self.doc_types = f.doc_types;
        self.form_for.clear();
        self.form = MetaForm::default();
        self.sort_rows();
    }

    fn sort_rows(&mut self) {
        let (col, desc) = self.sort;
        let clients = &self.clients;
        let key = |d: &DocRow| -> String {
            match col {
                Col::Name => d.name().to_lowercase(),
                Col::Client => client_of(clients, d).to_lowercase(),
                Col::Folder => folder_of(d).to_lowercase(),
                Col::DocType => d.doc_type.clone().unwrap_or_default().to_lowercase(),
                Col::Year => d.tax_year.map_or(String::new(), |y| format!("{y:05}")),
                Col::Status => d.status.clone(),
                Col::Source => d.source.clone().unwrap_or_default(),
                Col::Size => format!("{:020}", d.size),
            }
        };
        self.rows.sort_by(|a, b| {
            let o = key(a)
                .cmp(&key(b))
                .then_with(|| a.rel_path.cmp(&b.rel_path));
            if desc { o.reverse() } else { o }
        });
    }

    /// A change to the ticked documents, on its thread.
    fn run(
        &mut self,
        ctx: &egui::Context,
        env: &Env,
        catalog: &Path,
        what: &str,
        work: impl FnOnce(&mut Connection, &Path, &str, &Progress) -> Result<String, String>
        + Send
        + 'static,
    ) {
        let (catalog, root, device) = (
            catalog.to_path_buf(),
            env.root.to_path_buf(),
            env.device_id.to_string(),
        );
        self.progress = Default::default();
        let pr = self.progress.clone();
        match Job::start(ctx, what, move || {
            let mut conn = vf_core::db::open(&catalog, vf_core::db::Storage::Local)
                .map_err(|e| e.to_string())?;
            work(&mut conn, &root, &device, &pr)
        }) {
            Ok(j) => {
                self.work = Some(j);
                self.error = None;
                self.notice = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Takes a finished change, wherever the person is by then (the app calls this each frame):
    /// Browse must read the catalog again even when this page is not the one shown.
    pub fn poll(&mut self) {
        let Some(r) = job::take(&mut self.work) else {
            return;
        };
        match r.and_then(|r| r) {
            Ok(msg) => {
                self.notice = Some(msg);
                self.error = None;
                self.changed = true;
                self.picked.clear();
                self.renaming = None;
                self.refind = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn picked_ids(&self) -> Vec<String> {
        // In the grid's order, so a preview reads like the grid.
        self.rows
            .iter()
            .filter(|d| self.picked.contains(&d.id))
            .map(|d| d.id.clone())
            .collect()
    }
}

fn client_of<'a>(clients: &'a HashMap<String, String>, d: &DocRow) -> &'a str {
    d.client_id
        .as_deref()
        .and_then(|id| clients.get(id))
        .map_or("—", String::as_str)
}

/// The document's folder below its client's (or the cabinet's).
fn folder_of(d: &DocRow) -> String {
    let dir = d.rel_path.rsplit_once('/').map_or("", |(d, _)| d);
    match dir.split_once('/') {
        Some((_, rest)) if d.client_id.is_some() => rest.to_string(),
        _ if d.client_id.is_some() => String::new(),
        _ => dir.to_string(),
    }
}

fn count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn docs(n: usize) -> String {
    format!("{} document{}", count(n), if n == 1 { "" } else { "s" })
}

pub fn show(ui: &mut egui::Ui, p: &mut BulkPage, env: &mut Env, catalog: &Path) {
    let ctx = ui.ctx().clone();
    if let Some(r) = job::take(&mut p.finding) {
        match r.and_then(|r| r) {
            Ok(f) => p.found(f),
            Err(e) => p.error = Some(format!("The documents could not be listed: {e}")),
        }
    }
    p.poll();
    // The grid as the catalog now has it.
    if std::mem::take(&mut p.refind)
        && let Some(ph) = p.found_for.clone()
    {
        p.phrase = ph;
        p.find(&ctx, catalog);
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Bulk changes").strong().size(16.0));
        ui.label(widgets::caption(
            "Find documents by part of their file name, tick them, and change or trash them together.",
        ));
    });
    ui.add_space(6.0);
    let idle = p.finding.is_none() && p.work.is_none();
    ui.horizontal(|ui| {
        let l = ui.label("File name contains");
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut p.phrase).margin(crate::ui::widgets::SEARCH_MARGIN)
                    .hint_text("e.g. GovtK1")
                    .desired_width(260.0),
            )
            .labelled_by(l.id);
        widgets::clear_x(ui, &r, &mut p.phrase);
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let go = ui
            .add_enabled(idle, egui::Button::new("Find"))
            .on_hover_text(
                "Lists the documents whose file name has this text, capitals or not. Folder names are not looked at.",
            )
            .clicked();
        if (go || enter) && idle {
            p.find(&ctx, catalog);
        }
        if let Some(j) = &p.finding {
            j.show(ui);
        }
    });
    if let Some(j) = &p.work {
        use std::sync::atomic::Ordering::Relaxed;
        ui.horizontal(|ui| {
            j.show(ui);
            let (done, of) = (p.progress.done.load(Relaxed), p.progress.of.load(Relaxed));
            if of > PART {
                ui.label(format!("{} of {}", count(done), count(of)));
                let stopping = p.progress.stop.load(Relaxed);
                if ui
                    .add_enabled(!stopping, egui::Button::new("Cancel"))
                    .on_hover_text(format!(
                        "Stops after the {PART} documents being changed now. What is already changed stays changed."
                    ))
                    .clicked()
                {
                    p.progress.stop.store(true, Relaxed);
                }
                if stopping {
                    ui.label(widgets::caption("Stopping…"));
                }
            }
        });
    }
    if let Some(e) = &p.error {
        ui.colored_label(theme::error(), e);
    }
    if let Some(n) = &p.notice {
        ui.label(n);
    }
    let Some(found_for) = p.found_for.clone() else {
        return;
    };
    ui.add_space(6.0);
    // The ticked rows, and the form for them.
    if p.form_for != p.picked {
        let (sel, picked): (BTreeSet<usize>, Vec<&DocRow>) = p
            .rows
            .iter()
            .enumerate()
            .filter(|(_, d)| p.picked.contains(&d.id))
            .unzip();
        p.form = MetaForm::for_docs(&sel, &picked, &p.doc_types);
        p.form_for = p.picked.clone();
    }
    let n = p.picked.len();
    ui.horizontal_wrapped(|ui| {
        ui.label(if p.rows.is_empty() {
            format!("No document's file name contains \"{found_for}\".")
        } else if p.more {
            format!(
                "First {} shown; narrow the text for the rest. {} selected.",
                count(LIMIT),
                count(n)
            )
        } else {
            format!("{} · {} selected", docs(p.rows.len()), count(n))
        });
        if !p.rows.is_empty() {
            if ui.small_btn("Select all").clicked() {
                p.picked = p.rows.iter().map(|d| d.id.clone()).collect();
            }
            if ui.small_btn("Select none").clicked() {
                p.picked.clear();
            }
        }
    });
    if p.rows.is_empty() {
        return;
    }
    actions(ui, p, env, catalog, idle);
    ui.add_space(6.0);
    grid(ui, p);
}

/// The changes offered for the ticked rows.
fn actions(ui: &mut egui::Ui, p: &mut BulkPage, env: &mut Env, catalog: &Path, idle: bool) {
    let ctx = ui.ctx().clone();
    let n = p.picked.len();
    let can = idle && n > 0;
    egui::Frame::new()
        .fill(theme::pane())
        .stroke(egui::Stroke::new(1.0, theme::border_pane()))
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(widgets::caption(if n == 0 {
                "Tick documents to change them.".to_string()
            } else {
                format!("For the {} selected:", docs(n))
            }));
            ui.add_enabled_ui(can, |ui| {
                // As wide as Browse's details pane draws it.
                ui.set_max_width(420.0);
                meta_editor::fields(ui, &mut p.form, &p.doc_types);
            });
            let edit = p.form.to_edit();
            let touched = edit != vf_fs::docmeta::MetaEdit::default();
            let (mut apply, mut trash, mut rename) = (false, false, false);
            ui.horizontal_wrapped(|ui| {
                apply = ui
                    .add_enabled(
                        can && touched,
                        egui::Button::new(format!("Apply to {}", count(n))),
                    )
                    .on_hover_text("Sets the fields chosen above on every selected document (one transaction; Undo last in Browse takes it back).")
                    .on_disabled_hover_text("Choose a doc type, tax year, status, source or tags first.")
                    .clicked();
                ui.separator();
                rename = ui
                    .add_enabled(can, egui::Button::new("Rename…"))
                    .on_hover_text("Replace text in the selected documents' file names, with a preview first.")
                    .clicked();
                ui.separator();
                trash = ui
                    .add_enabled(can, egui::Button::new(format!("Move {} to trash", count(n))))
                    .on_hover_text("Admin › Trash restores them for 90 days.")
                    .clicked();
            });
            if apply {
                let ids = p.picked_ids();
                p.run(&ctx, env, catalog, "Changing the documents…", move |conn, root, dev, pr| {
                    let r = in_parts(&ids, pr, |part| {
                        match vf_fs::docmeta::edit(conn, root, part, &edit, dev) {
                            Ok((_, changed)) => Ok(changed),
                            // None of this part's documents differ: nothing to do for it.
                            Err(e) if e.contains("already have these values") => Ok(0),
                            Err(e) => Err(e),
                        }
                    })?;
                    if r.changed == 0 && !r.stopped {
                        return Err("The selected documents already have these values.".into());
                    }
                    Ok(said(
                        "changed",
                        &r,
                        ids.len(),
                        "One transaction: Undo last in Browse takes it back.",
                    ))
                });
            }
            if trash {
                p.confirm_trash = true;
            }
            if rename {
                p.renaming = Some(Renaming {
                    find: p.found_for.clone().unwrap_or_default(),
                    ..Default::default()
                });
            }
        });
    confirm_trash(ui, p, env, catalog);
    renaming(ui, p, env, catalog, idle);
}

fn confirm_trash(ui: &mut egui::Ui, p: &mut BulkPage, env: &mut Env, catalog: &Path) {
    if !p.confirm_trash {
        return;
    }
    let ctx = ui.ctx().clone();
    let n = p.picked.len();
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("bulk-trash")).show(&ctx, |ui| {
        ui.set_width(440.0);
        ui.heading(format!("Move {} to the trash?", docs(n)));
        for d in p.rows.iter().filter(|d| p.picked.contains(&d.id)).take(5) {
            ui.label(RichText::new(d.name()).family(theme::mono_semibold(ui)));
        }
        if n > 5 {
            ui.label(widgets::caption(format!("and {} more", count(n - 5))));
        }
        ui.add_space(6.0);
        ui.label("The files leave their folders. Admin › Trash restores them for 90 days; after that they are deleted for good.");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .button(format!("Yes, move {} to trash", count(n)))
                .clicked()
            {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(false);
            }
        });
    });
    match answer {
        Some(true) => {
            p.confirm_trash = false;
            let ids = p.picked_ids();
            p.run(
                &ctx,
                env,
                catalog,
                "Moving to the trash…",
                move |conn, root, dev, pr| {
                    let r = in_parts(&ids, pr, |part| {
                        vf_fs::trash::delete(conn, root, dev, part, "deleted").map(|_| part.len())
                    })?;
                    Ok(said(
                        "moved to the trash",
                        &r,
                        ids.len(),
                        "Admin › Trash restores them.",
                    ))
                },
            );
        }
        Some(false) => p.confirm_trash = false,
        None => {}
    }
}

/// The find-and-replace in the ticked documents' names: the two texts, a preview, then Rename.
fn renaming(ui: &mut egui::Ui, p: &mut BulkPage, env: &mut Env, catalog: &Path, idle: bool) {
    let ctx = ui.ctx().clone();
    if p.renaming.is_none() {
        return;
    }
    let ticked = p.picked.len();
    // The preview stands only for the documents ticked when it was made.
    let same_ticks = p
        .renaming
        .as_ref()
        .is_some_and(|r| r.planned_ids == p.picked);
    let Some(r) = p.renaming.as_mut() else {
        return;
    };
    if let Some(done) = job::take(&mut r.job) {
        match done.and_then(|d| d) {
            Ok(rows) => {
                r.plan = Some(rows);
                r.error = None;
            }
            Err(e) => {
                r.plan = None;
                r.error = Some(e);
            }
        }
    }
    let (mut preview, mut go, mut close) = (false, false, false);
    ui.add_space(6.0);
    egui::Frame::new()
        .fill(theme::pane())
        .stroke(egui::Stroke::new(1.0, theme::border_pane()))
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("Rename {}", docs(ticked))).strong());
            ui.horizontal_wrapped(|ui| {
                let l = ui.label("Replace");
                ui.add(egui::TextEdit::singleline(&mut r.find).desired_width(180.0))
                    .labelled_by(l.id);
                let l = ui.label("with");
                ui.add(
                    egui::TextEdit::singleline(&mut r.replace)
                        .hint_text("(nothing: the text is taken out)")
                        .desired_width(180.0),
                )
                .labelled_by(l.id);
                preview = ui
                    .add_enabled(r.job.is_none() && idle, egui::Button::new("Preview"))
                    .on_hover_text(
                        "Shows each new name; nothing is renamed yet. The extension is kept.",
                    )
                    .clicked();
                close = ui.button("Close").clicked();
                if let Some(j) = &r.job {
                    j.show(ui);
                }
            });
            if let Some(e) = &r.error {
                ui.colored_label(theme::error(), e);
            }
            let current = same_ticks && r.planned_for == (r.find.clone(), r.replace.clone());
            if r.plan.is_some() && !current {
                ui.label(widgets::caption(
                    "The text or the ticked documents changed: Preview again.",
                ));
            }
            if let Some(plan) = r.plan.as_ref().filter(|_| current) {
                let will = plan.iter().filter(|x| x.renames()).count();
                ui.label(widgets::caption(format!(
                    "{} will be renamed, {} left as they are.",
                    docs(will),
                    count(plan.len() - will)
                )));
                egui::ScrollArea::vertical()
                    .id_salt("bulk-rename-preview")
                    .max_height(160.0)
                    .show_rows(ui, 20.0, plan.len(), |ui, range| {
                        for x in &plan[range] {
                            let old = x.old_rel.rsplit('/').next().unwrap_or(&x.old_rel);
                            let name = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(old).family(theme::mono_semibold(ui)));
                                match &x.outcome {
                                    Outcome::Rename(new) => {
                                        ui.label("→");
                                        ui.label(name(new));
                                    }
                                    Outcome::KeepBoth(new) => {
                                        ui.label("→");
                                        ui.label(name(new));
                                        ui.label(widgets::caption("(that name is taken)"));
                                    }
                                    Outcome::Same => {
                                        ui.label(widgets::caption(
                                            "not changed: the text is not in its name",
                                        ));
                                    }
                                    Outcome::Skip(why) => {
                                        ui.label(
                                            RichText::new(format!("not renamed: {why}"))
                                                .small()
                                                .color(theme::warn()),
                                        );
                                    }
                                }
                            });
                        }
                    });
                go = ui
                    .add_enabled(
                        will > 0 && idle,
                        egui::Button::new(format!("Rename {}", count(will))),
                    )
                    .on_hover_text("One transaction; Undo last in Browse takes it back.")
                    .clicked();
            }
        });
    if close {
        p.renaming = None;
        return;
    }
    if preview {
        let (find, replace) = (r.find.clone(), r.replace.clone());
        r.planned_for = (find.clone(), replace.clone());
        r.planned_ids = p.picked.clone();
        r.plan = None;
        // In the grid's order, so the preview reads like the grid.
        let ids: Vec<String> = p
            .rows
            .iter()
            .filter(|d| p.picked.contains(&d.id))
            .map(|d| d.id.clone())
            .collect();
        let (catalog, root) = (catalog.to_path_buf(), env.root.to_path_buf());
        let year = chrono::Datelike::year(&chrono::Local::now());
        match Job::start(&ctx, "Working out the new names…", move || {
            let conn = vf_core::db::open(&catalog, vf_core::db::Storage::Local)
                .map_err(|e| e.to_string())?;
            vf_fs::rename::Planner::load(&conn, &root, &ids, year)?
                .plan_replace(&root, &find, &replace)
        }) {
            Ok(j) => r.job = Some(j),
            Err(e) => r.error = Some(e),
        }
    } else if go
        && same_ticks
        && let Some(rows) = r.plan.clone()
    {
        p.run(
            &ctx,
            env,
            catalog,
            "Renaming…",
            move |conn, root, dev, pr| {
                // Only the rows that rename: a part of rows that all stay is not a transaction.
                let rows: Vec<Row> = rows.into_iter().filter(Row::renames).collect();
                let r = in_parts(&rows, pr, |part| {
                    vf_fs::rename::commit(conn, root, part, dev).map(|(_, n)| n)
                })?;
                Ok(said(
                    "renamed",
                    &r,
                    rows.len(),
                    "One transaction: Undo last in Browse takes it back.",
                ))
            },
        );
    }
}

fn grid(ui: &mut egui::Ui, p: &mut BulkPage) {
    let mut sort = None;
    let mut toggled: Option<(String, bool)> = None;
    let mut all: Option<bool> = None;
    let every = !p.rows.is_empty() && p.picked.len() == p.rows.len();
    // A ticked row keeps the normal text color; its fill shows the tick (as in Browse).
    ui.visuals_mut().selection.stroke.color = theme::ink();
    egui::ScrollArea::horizontal()
        .id_salt("bulk-table-x")
        .show(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .striped(true)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::exact(24.0));
            for (c, _) in COLS {
                table = table.column(match c {
                    Col::Name => Column::initial(300.0)
                        .at_least(120.0)
                        .clip(true)
                        .resizable(true),
                    Col::Client | Col::Folder => Column::initial(180.0)
                        .at_least(80.0)
                        .clip(true)
                        .resizable(true),
                    Col::DocType => Column::initial(120.0)
                        .at_least(60.0)
                        .clip(true)
                        .resizable(true),
                    _ => Column::initial(76.0)
                        .at_least(50.0)
                        .clip(true)
                        .resizable(true),
                });
            }
            table
                .header(28.0, |mut h| {
                    h.col(|ui| {
                        let mut on = every;
                        let r = ui.add(egui::Checkbox::without_text(&mut on));
                        r.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::accesskit::Role::CheckBox,
                                true,
                                every,
                                "Select every row",
                            )
                        });
                        if r.changed() {
                            all = Some(on);
                        }
                    });
                    for (col, name) in COLS {
                        h.col(|ui| {
                            let t = RichText::new(name.to_uppercase())
                                .small()
                                .color(theme::ink_muted());
                            if ui.add(egui::Button::new(t).frame(false)).clicked() {
                                sort = Some(col);
                            }
                            if p.sort.0 == col {
                                crate::ui::browse::sort_arrow(ui, !p.sort.1);
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(ROW_H, p.rows.len(), |mut row| {
                        let d = &p.rows[row.index()];
                        let mut sel = p.picked.contains(&d.id);
                        row.set_selected(sel);
                        row.col(|ui| {
                            let r = ui.add(egui::Checkbox::without_text(&mut sel));
                            let now = sel;
                            r.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::accesskit::Role::CheckBox,
                                    true,
                                    now,
                                    format!("Select {}", d.name()),
                                )
                            });
                            if r.changed() {
                                toggled = Some((d.id.clone(), sel));
                            }
                        });
                        for (col, _) in COLS {
                            row.col(|ui| {
                                match col {
                                    Col::Name => ui
                                        .label(
                                            RichText::new(d.name())
                                                .family(theme::mono_semibold(ui))
                                                .color(theme::ink()),
                                        )
                                        .on_hover_text(&d.rel_path),
                                    Col::Client => ui.label(client_of(&p.clients, d)),
                                    Col::Folder => ui.label(folder_of(d)),
                                    Col::DocType => ui.label(d.doc_type.as_deref().unwrap_or("—")),
                                    Col::Year => ui.label(widgets::year_label(d.tax_year)),
                                    Col::Status => ui.label(widgets::label_of(
                                        &vf_fs::docmeta::STATUSES,
                                        Some(&d.status),
                                        "—",
                                    )),
                                    Col::Source => ui.label(widgets::label_of(
                                        &vf_fs::docmeta::SOURCES,
                                        d.source.as_deref(),
                                        "—",
                                    )),
                                    Col::Size => ui.label(widgets::size(d.size)),
                                };
                            });
                        }
                    });
                });
        });
    if let Some((id, on)) = toggled {
        if on {
            p.picked.insert(id);
        } else {
            p.picked.remove(&id);
        }
    }
    match all {
        Some(true) => p.picked = p.rows.iter().map(|d| d.id.clone()).collect(),
        Some(false) => p.picked.clear(),
        None => {}
    }
    if let Some(col) = sort {
        p.sort = if p.sort.0 == col {
            (col, !p.sort.1)
        } else {
            (col, false)
        };
        p.sort_rows();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering::Relaxed;

    use super::*;

    #[test]
    fn a_large_change_is_made_in_parts_and_can_be_stopped() {
        let items: Vec<usize> = (0..PART * 2 + 7).collect();
        let pr = Progress::default();
        let r = in_parts(&items, &pr, |part| Ok(part.len())).unwrap();
        assert_eq!((r.changed, r.parts, r.stopped), (PART * 2 + 7, 3, false));
        assert_eq!(pr.done.load(Relaxed), items.len());
        assert_eq!(pr.of.load(Relaxed), items.len());
        assert!(
            said("renamed", &r, items.len(), "x")
                .starts_with("Renamed 1,007 documents. Done in 3 transactions")
        );

        // Asked to stop during the first part: that part is finished, no other is begun.
        let pr = Progress::default();
        let r = in_parts(&items, &pr, |part| {
            pr.stop.store(true, Relaxed);
            Ok(part.len())
        })
        .unwrap();
        assert_eq!((r.changed, r.parts, r.stopped), (PART, 1, true));
        assert_eq!(
            said(
                "moved to the trash",
                &r,
                items.len(),
                "Admin › Trash restores them."
            ),
            "Stopped: moved to the trash 500 of 1,007 documents. Admin › Trash restores them."
        );

        // A part that fails after others were done says how many were.
        let pr = Progress::default();
        let mut n = 0;
        let e = in_parts(&items, &pr, |part| {
            n += 1;
            if n == 2 {
                Err("the share went away".to_string())
            } else {
                Ok(part.len())
            }
        })
        .err()
        .unwrap();
        assert!(
            e.starts_with("the share went away (500 documents were changed before this"),
            "{e}"
        );

        // A small change is one transaction, said as before.
        let pr = Progress::default();
        let r = in_parts(&items[..3], &pr, |part| Ok(part.len())).unwrap();
        assert_eq!(
            said("changed", &r, 3, "One transaction."),
            "Changed 3 documents. One transaction."
        );
    }
}
