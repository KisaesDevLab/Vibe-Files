//! Admin › Clients from a list (owner 2026-11-17, Q-131, PA-94): the firm's list of client
//! names and IDs (a CSV with those two columns) put onto the cabinet's clients. Each row is
//! matched to a client by its ID, else by how closely the names agree; a person ticks the
//! matches that are right, sees what each would do, and the ticked clients take the list's ID
//! and name, their folders renamed with them. Nothing is ticked for them.
//!
//! Reading the list, matching (every row against every client), the preview, the run and its
//! undo are on a thread with their own catalog connection. After a run the changed clients'
//! rows leave the list; the list is matched again only when the person presses Find.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

use egui::{Align, Layout, RichText};
use egui_extras::{Column, TableBuilder};
use vf_core::db::Connection;
use vf_fs::listupdate::{self, Change, Columns, Found, Kind, Match, Outcome, Previewed, Run};
use vf_fs::namematch::Tier;

use crate::ui::browse::Env;
use crate::ui::job::{self, Job};
use crate::ui::theme;
use crate::ui::widgets::{self, UiExt as _};

const ROW_H: f32 = 26.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Group {
    #[default]
    All,
    /// By how sure the match is.
    Exact,
    Rule,
    Near,
    /// By what the client has now.
    NoId,
    OtherId,
    SameId,
}

#[derive(Default)]
struct Progress {
    done: AtomicUsize,
    stop: AtomicBool,
}

/// What Find brings back: the matches, how many rows had a blank, and the last run that can
/// still be put back.
struct Looked {
    found: Found,
    blank: usize,
    last: Option<Run>,
}

/// What the ticked changes would do, or why they could not be worked out.
type Planned = Result<Vec<Previewed>, String>;
type Ran = (Run, Vec<(Change, Outcome)>);
type PutBack = Vec<(String, Result<(), String>)>;

pub struct ListUpdatePage {
    pub path: String,
    columns: Option<Columns>,
    reading: Option<Job<Result<Columns, String>>>,
    id_col: String,
    name_col: String,
    /// The name match asked for, in percent.
    pct: f64,
    found: Option<Found>,
    blank: usize,
    finding: Option<Job<Result<Result<Looked, String>, String>>>,
    filter: String,
    group: Group,
    /// Ticked matches, by client uuid.
    picked: BTreeSet<String>,
    previewing: Option<Job<Result<Planned, String>>>,
    preview: Option<Vec<Previewed>>,
    work: Option<Job<Result<Ran, String>>>,
    progress: Arc<Progress>,
    of: usize,
    /// The last run with changes to put back (this one's, or one found in the cabinet).
    last_run: Option<Run>,
    undoing: Option<Job<Result<PutBack, String>>>,
    /// What the last run or undo did, client by client (the ones that need a look).
    results: Vec<String>,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The catalog changed: Browse re-reads.
    pub changed: bool,
}

impl Default for ListUpdatePage {
    fn default() -> Self {
        ListUpdatePage {
            path: String::new(),
            columns: None,
            reading: None,
            id_col: String::new(),
            name_col: String::new(),
            pct: listupdate::NAME_MATCH * 100.0,
            found: None,
            blank: 0,
            finding: None,
            filter: String::new(),
            group: Group::All,
            picked: BTreeSet::new(),
            previewing: None,
            preview: None,
            work: None,
            progress: Default::default(),
            of: 0,
            last_run: None,
            undoing: None,
            results: vec![],
            notice: None,
            error: None,
            changed: false,
        }
    }
}

fn clients(n: usize) -> String {
    format!("{n} client{}", if n == 1 { "" } else { "s" })
}

fn id_text(id: &str) -> &str {
    if id.trim().is_empty() { "no ID" } else { id }
}

impl ListUpdatePage {
    /// Whether the list is being matched (tests).
    #[cfg(test)]
    pub fn is_finding(&self) -> bool {
        self.finding.is_some()
    }

    /// Show set to "Has another ID" (tests).
    #[cfg(test)]
    pub fn show_other_ids(&mut self) {
        self.group = Group::OtherId;
    }

    /// The matches the filter and the group show, by their place in the found list.
    fn shown(&self) -> Vec<usize> {
        let Some(f) = &self.found else {
            return vec![];
        };
        let q = self.filter.trim().to_lowercase();
        f.matches
            .iter()
            .enumerate()
            .filter(|(_, m)| match self.group {
                Group::All => true,
                Group::Exact => m.tier == Tier::Exact,
                Group::Rule => m.tier == Tier::Rule,
                Group::Near => m.tier == Tier::Near,
                Group::NoId => m.kind == Kind::NoId,
                Group::OtherId => m.kind == Kind::OtherId,
                Group::SameId => m.kind == Kind::SameId,
            })
            .filter(|(_, m)| {
                q.is_empty()
                    || [&m.row.name, &m.row.id, &m.client.name, &m.client.ext_id]
                        .iter()
                        .any(|s| s.to_lowercase().contains(&q))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The ticked matches as changes, in the list's order.
    fn changes(&self) -> Vec<Change> {
        self.found
            .iter()
            .flat_map(|f| &f.matches)
            .filter(|m| self.picked.contains(&m.client.id))
            .map(|m| Change {
                client: m.client.id.clone(),
                id: m.row.id.clone(),
                name: m.row.name.clone(),
            })
            .collect()
    }

    /// Takes a finished run or undo, whichever tab is shown (the app calls this each frame):
    /// Browse must read the catalog again.
    pub fn poll(&mut self) {
        if let Some(r) = job::take(&mut self.work) {
            match r.and_then(|r| r) {
                Ok((run, out)) => self.ran(run, out),
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = job::take(&mut self.undoing) {
            match r.and_then(|r| r) {
                Ok(back) => {
                    let ok = back.iter().filter(|(_, r)| r.is_ok()).count();
                    self.results = back
                        .iter()
                        .filter_map(|(c, r)| r.as_ref().err().map(|e| format!("{c}: {e}")))
                        .collect();
                    // Put back from the end of the run; it stops at the first that cannot be.
                    let left = self.last_run.as_mut().map_or(0, |run| {
                        let keep = run.done.len().saturating_sub(ok);
                        run.done.truncate(keep);
                        keep
                    });
                    if left == 0 {
                        self.last_run = None;
                    }
                    self.notice = Some(format!(
                        "{} put back.{} The list below is not matched again until you press Find matches.",
                        clients(ok),
                        if left == 0 {
                            String::new()
                        } else {
                            format!(
                                " Stopped there: {} still changed (the reason is below).",
                                clients(left)
                            )
                        }
                    ));
                    self.changed = true;
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    fn ran(&mut self, run: Run, out: Vec<(Change, Outcome)>) {
        let mut changed: BTreeSet<String> = BTreeSet::new();
        self.results.clear();
        for (c, o) in out {
            match o {
                Outcome::Changed(note) => {
                    if let Some(n) = note {
                        self.results.push(format!("{} ({}): {n}", c.name, c.id));
                    }
                    changed.insert(c.client);
                }
                Outcome::Refused(why) => self
                    .results
                    .push(format!("{} ({}): not changed. {why}", c.name, c.id)),
            }
        }
        self.notice = Some(format!(
            "Updated {}. The list below is not matched again until you press Find matches.{}",
            clients(changed.len()),
            if self.results.is_empty() {
                ""
            } else {
                " These need a look:"
            }
        ));
        self.error = None;
        // The changed clients' rows leave the list (owner 2026-11-17: nothing is searched
        // again after an action until asked).
        if let Some(f) = &mut self.found {
            f.matches.retain(|m| !changed.contains(&m.client.id));
            f.already += changed.len();
        }
        // A run that changed nothing leaves the last run's Undo as it was.
        if !run.done.is_empty() {
            self.last_run = Some(run);
        }
        self.picked.clear();
        self.changed = true;
    }
}

/// `work` on its own thread with its own catalog connection.
fn on_thread<T: Send + 'static>(
    ctx: &egui::Context,
    env: &Env,
    catalog: &Path,
    what: &str,
    work: impl FnOnce(&mut Connection, &Path, &str) -> T + Send + 'static,
) -> Result<Job<Result<T, String>>, String> {
    let (catalog, root, device) = (
        catalog.to_path_buf(),
        env.root.to_path_buf(),
        env.device_id.to_string(),
    );
    Job::start(ctx, what, move || {
        let mut conn =
            vf_core::db::open(&catalog, vf_core::db::Storage::Local).map_err(|e| e.to_string())?;
        Ok(work(&mut conn, &root, &device))
    })
}

fn column_box(ui: &mut egui::Ui, label: &str, salt: &str, headers: &[String], col: &mut String) {
    let l = ui.label(label);
    egui::ComboBox::from_id_salt(salt)
        .selected_text(if col.is_empty() {
            "Choose…"
        } else {
            col.as_str()
        })
        .show_ui(ui, |ui| {
            for h in headers {
                ui.selectable_value(col, h.clone(), h);
            }
        })
        .response
        .labelled_by(l.id);
}

pub fn show(ui: &mut egui::Ui, p: &mut ListUpdatePage, env: &mut Env, catalog: &Path) {
    let ctx = ui.ctx().clone();
    if let Some(r) = job::take(&mut p.reading) {
        match r.and_then(|r| r) {
            Ok(c) => {
                p.id_col = c.id.clone().unwrap_or_default();
                p.name_col = c.name.clone().unwrap_or_default();
                p.columns = Some(c);
                p.error = None;
            }
            Err(e) => {
                p.columns = None;
                p.error = Some(format!("The list could not be read: {e}"));
            }
        }
    }
    if let Some(r) = job::take(&mut p.finding) {
        match r.and_then(|r| r).and_then(|r| r) {
            Ok(l) => {
                let live: BTreeSet<&str> = l
                    .found
                    .matches
                    .iter()
                    .map(|m| m.client.id.as_str())
                    .collect();
                p.picked.retain(|k| live.contains(k.as_str()));
                p.found = Some(l.found);
                p.blank = l.blank;
                // A run found in the cabinet (made on another computer, or before Vibe Files
                // was closed) can be put back from here too.
                if p.last_run.is_none() {
                    p.last_run = l.last;
                }
            }
            Err(e) => p.error = Some(format!("The list could not be matched: {e}")),
        }
    }
    if let Some(r) = job::take(&mut p.previewing) {
        match r.and_then(|r| r).and_then(|r| r) {
            Ok(pv) => p.preview = Some(pv),
            Err(e) => p.error = Some(e),
        }
    }
    p.poll();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Clients from a list").strong().size(16.0));
        ui.label(widgets::caption(
            "A list of client names and IDs (CSV) put onto the cabinet's clients: each matched client takes the list's ID and name, and its folder is renamed. Tick the matches that are right.",
        ));
    });
    ui.add_space(6.0);
    let idle = p.reading.is_none()
        && p.finding.is_none()
        && p.previewing.is_none()
        && p.work.is_none()
        && p.undoing.is_none();
    ui.horizontal(|ui| {
        let l = ui.label("List (CSV)");
        ui.add(egui::TextEdit::singleline(&mut p.path).desired_width(360.0))
            .labelled_by(l.id);
        if ui.add_enabled(idle, egui::Button::new("Choose…")).clicked()
            && let Some(f) = rfd::FileDialog::new()
                .add_filter("CSV", &["csv", "txt"])
                .pick_file()
        {
            p.path = f.display().to_string();
        }
        if ui
            .add_enabled(
                idle && !p.path.trim().is_empty(),
                egui::Button::new("Read the list"),
            )
            .on_hover_text("Reads the list's columns. Its first row is their names.")
            .clicked()
        {
            let path = PathBuf::from(p.path.trim());
            match Job::start(&ctx, "Reading the list…", move || {
                listupdate::columns(&path)
            }) {
                Ok(j) => p.reading = Some(j),
                Err(e) => p.error = Some(e),
            }
        }
        if let Some(j) = &p.reading {
            j.show(ui);
        }
    });
    if let Some(cols) = p.columns.clone() {
        ui.horizontal(|ui| {
            column_box(ui, "ID column", "listupdate-id", &cols.headers, &mut p.id_col);
            column_box(
                ui,
                "Name column",
                "listupdate-name",
                &cols.headers,
                &mut p.name_col,
            );
            let l = ui.label("Names match at least");
            ui.add(
                egui::DragValue::new(&mut p.pct)
                    .range(80.0..=100.0)
                    .speed(0.1)
                    .max_decimals(0)
                    .suffix(" %"),
            )
            .labelled_by(l.id)
            .on_hover_text("The same name is always a match, however it is written (\"&\" or \"and\", \"Inc\" or \"Incorporated\", a middle initial, the words in another order). This number is for names that are only close (a word shortened, a number or \"Jr\" on one side): lower it to see more of those. They are marked, and ticked one at a time.");
            let ready = !p.id_col.is_empty() && !p.name_col.is_empty() && p.id_col != p.name_col;
            if ui
                .add_enabled(idle && ready, egui::Button::new("Find matches"))
                .on_hover_text("Matches every row of the list to the cabinet's clients. Nothing is changed.")
                .on_disabled_hover_text("Choose the ID column and the name column.")
                .clicked()
            {
                p.notice = None;
                p.results.clear();
                let (path, id, name, pct) = (
                    PathBuf::from(p.path.trim()),
                    p.id_col.clone(),
                    p.name_col.clone(),
                    p.pct / 100.0,
                );
                match on_thread(
                    &ctx,
                    env,
                    catalog,
                    "Matching the list to the clients…",
                    move |conn, root, _| -> Result<Looked, String> {
                        let (rows, blank) = listupdate::read_rows(&path, &id, &name)?;
                        Ok(Looked {
                            found: listupdate::find(conn, Some(root), &rows, pct)?,
                            blank,
                            last: listupdate::load_last(root)?,
                        })
                    },
                ) {
                    Ok(j) => {
                        p.finding = Some(j);
                        p.error = None;
                    }
                    Err(e) => p.error = Some(e),
                }
            }
            if let Some(j) = &p.finding {
                j.show(ui);
            }
            for j in [p.previewing.as_ref().map(|j| j as &dyn Shown)]
                .into_iter()
                .chain([p.undoing.as_ref().map(|j| j as &dyn Shown)])
                .flatten()
            {
                j.draw(ui);
            }
        });
    }
    if let Some(j) = &p.work {
        ui.horizontal(|ui| {
            j.show(ui);
            ui.label(format!("{} of {}", p.progress.done.load(Relaxed), p.of));
            let stopping = p.progress.stop.load(Relaxed);
            if ui
                .add_enabled(!stopping, egui::Button::new("Cancel"))
                .on_hover_text("Stops after the client being changed now. What is changed stays changed; Undo this update takes it back.")
                .clicked()
            {
                p.progress.stop.store(true, Relaxed);
            }
            if stopping {
                ui.label(widgets::caption("Stopping…"));
            }
        });
    }
    if let Some(e) = &p.error {
        ui.colored_label(theme::error(), e);
    }
    if let Some(n) = &p.notice {
        ui.label(n);
    }
    if !p.results.is_empty() {
        egui::ScrollArea::vertical()
            .id_salt("listupdate-results")
            .max_height(110.0)
            .show(ui, |ui| {
                for r in &p.results {
                    ui.label(widgets::caption(r));
                }
            });
    }
    if let Some(run) = p.last_run.clone() {
        let n = run.done.len();
        if ui
            .add_enabled(
                idle,
                egui::Button::new(format!("Undo the last update ({})", clients(n))),
            )
            .on_hover_text("Gives the clients of the last update the ID, name and folder they had, the last one first. The client CSV is left as it is.")
            .clicked()
        {
            p.progress = Default::default();
            p.of = n;
            let pr = p.progress.clone();
            match on_thread(&ctx, env, catalog, "Putting the clients back…", move |conn, root, dev| {
                listupdate::undo(conn, root, dev, &run, &pr.done)
            }) {
                Ok(j) => p.undoing = Some(j),
                Err(e) => p.error = Some(e),
            }
        }
    }
    let Some(f) = &p.found else {
        return;
    };
    ui.add_space(6.0);
    let tier = |t: Tier| f.matches.iter().filter(|m| m.tier == t).count();
    ui.label(format!(
        "{} rows on the list · {} matches ({} same name, {} written differently, {} close) · {} right already · {} ticked",
        f.rows,
        f.matches.len(),
        tier(Tier::Exact),
        tier(Tier::Rule),
        tier(Tier::Near),
        f.already,
        p.picked.len()
    ));
    // The discrepancies, both ways.
    ui.label(format!(
        "Not matched: {} rows of the list have no client · {} clients are not on the list · {} folders have no client file{}",
        f.no_client.len(),
        f.no_row.len(),
        f.unbound.len(),
        if p.blank > 0 {
            format!(" · {} rows with a blank left out", p.blank)
        } else {
            String::new()
        },
    ));
    ui.horizontal(|ui| {
        if ui
            .small_btn("Save the comparison as CSV…")
            .on_hover_text("Every match, every row with no client and every client or folder with no row, each with the nearest thing to it and how close that is, to open in Excel.")
            .clicked()
            && let Some(to) = rfd::FileDialog::new()
                .add_filter("CSV", &["csv"])
                .set_file_name("client list comparison.csv")
                .save_file()
        {
            // With the mark Excel needs to read the accents.
            let text = format!("\u{feff}{}", listupdate::report_csv(f));
            match std::fs::write(&to, text) {
                Ok(()) => p.notice = Some(format!("Saved {}.", to.display())),
                Err(e) => p.error = Some(format!("{} could not be written: {e}", to.display())),
            }
        }
        ui.label(widgets::caption(
            "Rows with no client are not added here: Browse › Import clients CSV… adds the list's clients.",
        ));
    });
    let pct = |s: f64| format!("{:.0}%", (s * 100.0).floor());
    if !f.no_client.is_empty() {
        egui::CollapsingHeader::new(format!("On the list, no client ({})", f.no_client.len()))
            .id_salt("listupdate-no-client")
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("listupdate-no-client-list")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for (r, n) in f.no_client.iter().take(500) {
                            ui.label(widgets::caption(match n {
                                Some(n) if n.unbound => format!(
                                    "{} ({}) · nearest: the folder {} ({}), which has no client file",
                                    r.name,
                                    r.id,
                                    n.name,
                                    pct(n.score)
                                ),
                                Some(n) => format!(
                                    "{} ({}) · nearest: {} ({}), {}",
                                    r.name,
                                    r.id,
                                    n.name,
                                    id_text(&n.id),
                                    pct(n.score)
                                ),
                                None => format!("{} ({}) · nothing like it", r.name, r.id),
                            }));
                        }
                        if f.no_client.len() > 500 {
                            ui.label(widgets::caption("The first 500 are shown; the saved comparison has all."));
                        }
                    });
            });
    }
    if !f.no_row.is_empty() {
        egui::CollapsingHeader::new(format!("Clients not on the list ({})", f.no_row.len()))
            .id_salt("listupdate-no-row")
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("listupdate-no-row-list")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for (c, n) in f.no_row.iter().take(500) {
                            ui.label(widgets::caption(match n {
                                Some((r, s)) => format!(
                                    "{} ({}) · nearest row: {} ({}), {}",
                                    c.name,
                                    id_text(&c.ext_id),
                                    r.name,
                                    r.id,
                                    pct(*s)
                                ),
                                None => format!(
                                    "{} ({}) · nothing like it on the list",
                                    c.name,
                                    id_text(&c.ext_id)
                                ),
                            }));
                        }
                        if f.no_row.len() > 500 {
                            ui.label(widgets::caption(
                                "The first 500 are shown; the saved comparison has all.",
                            ));
                        }
                    });
            });
    }
    if !f.unclear.is_empty() {
        let text = f.unclear.join("\n");
        egui::CollapsingHeader::new(format!(
            "{} not changed: more than one match ({})",
            if f.unclear.len() == 1 {
                "1 is"
            } else {
                "Some are"
            },
            f.unclear.len()
        ))
        .id_salt("listupdate-unclear")
        .show(ui, |ui| {
            if ui
                .small_btn("Copy")
                .on_hover_text("Copy these lines")
                .clicked()
            {
                ui.ctx().copy_text(text.clone());
            }
            egui::ScrollArea::vertical()
                .id_salt("listupdate-unclear-list")
                .max_height(140.0)
                .show(ui, |ui| {
                    for u in &f.unclear {
                        ui.label(widgets::caption(u));
                    }
                });
        });
    }
    if f.matches.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        let l = ui.label("Show");
        egui::ComboBox::from_id_salt("listupdate-group")
            .selected_text(match p.group {
                Group::All => "All matches",
                Group::Exact => "Same name",
                Group::Rule => "Same name, written differently",
                Group::Near => "Close: look",
                Group::NoId => "No ID yet",
                Group::OtherId => "Has another ID",
                Group::SameId => "Same ID, another spelling",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut p.group, Group::All, "All matches");
                ui.selectable_value(&mut p.group, Group::Exact, "Same name");
                ui.selectable_value(&mut p.group, Group::Rule, "Same name, written differently");
                ui.selectable_value(&mut p.group, Group::Near, "Close: look");
                ui.selectable_value(&mut p.group, Group::NoId, "No ID yet");
                ui.selectable_value(&mut p.group, Group::OtherId, "Has another ID");
                ui.selectable_value(&mut p.group, Group::SameId, "Same ID, another spelling");
            })
            .response
            .labelled_by(l.id);
        let l = ui.label("Name or ID contains");
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut p.filter)
                    .margin(widgets::SEARCH_MARGIN)
                    .desired_width(200.0),
            )
            .labelled_by(l.id);
        widgets::clear_x(ui, &r, &mut p.filter);
        let shown = p.shown();
        if ui
            .small_btn("Tick the ones shown")
            .on_hover_text("Ticks the matches shown whose name is the same. Names written differently are ticked only when Show is set to them, and clients that have another ID only when Show is set to those: look down the list first. Close matches are never ticked for you.")
            .clicked()
            && let Some(f) = &p.found
        {
            let group = p.group;
            let add: Vec<String> = shown
                .iter()
                .map(|&i| &f.matches[i])
                .filter(|m| bulk_tickable(m, group))
                .map(|m| m.client.id.clone())
                .collect();
            p.picked.extend(add);
        }
        if ui.small_btn("Tick none").clicked() {
            p.picked.clear();
        }
    });
    let n = p.picked.len();
    if ui
        .add_enabled(
            idle && n > 0,
            egui::Button::new(format!("Update the {} ticked…", clients(n))),
        )
        .on_hover_text(
            "Shows what each change would do first. Nothing is changed until you say yes.",
        )
        .on_disabled_hover_text("Tick the matches that are right.")
        .clicked()
    {
        let changes = p.changes();
        match on_thread(
            &ctx,
            env,
            catalog,
            "Working out the changes…",
            move |conn, root, _| listupdate::preview(conn, root, &changes),
        ) {
            Ok(j) => {
                p.previewing = Some(j);
                p.error = None;
            }
            Err(e) => p.error = Some(e),
        }
    }
    confirm(ui, p, env, catalog);
    ui.add_space(6.0);
    // Not while a run or an undo is being made: a tick made then would be wiped when it ends.
    ui.add_enabled_ui(idle, |ui| grid(ui, p));
}

/// A job of any result, to draw its spinner.
trait Shown {
    fn draw(&self, ui: &mut egui::Ui);
}

impl<T: Send + 'static> Shown for Job<T> {
    fn draw(&self, ui: &mut egui::Ui) {
        self.show(ui);
    }
}

/// What the ticked changes would do, for a yes or no.
fn confirm(ui: &mut egui::Ui, p: &mut ListUpdatePage, env: &mut Env, catalog: &Path) {
    let Some(pv) = &p.preview else {
        return;
    };
    let ctx = ui.ctx().clone();
    let ok: Vec<&Previewed> = pv.iter().filter(|(_, r)| r.is_ok()).collect();
    let refused: Vec<&Previewed> = pv.iter().filter(|(_, r)| r.is_err()).collect();
    let folders = ok
        .iter()
        .filter(|(_, r)| r.as_ref().is_ok_and(|x| x.folder.is_some()))
        .count();
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("listupdate-confirm")).show(&ctx, |ui| {
        ui.set_width(680.0);
        ui.heading(format!("Update {}?", clients(ok.len())));
        ui.label(format!(
            "Each takes the list's ID and name on the client list, in the catalog and in its client file. {} renamed. Documents stay in their folders. Undo the last update puts all of it back.",
            if folders == 1 {
                "1 folder is".to_string()
            } else {
                format!("{folders} folders are")
            }
        ));
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("listupdate-preview")
            .max_height(280.0)
            .show(ui, |ui| {
                for (c, r) in &ok {
                    let Ok(x) = r else { continue };
                    ui.label(format!(
                        "{} ({})  →  {} ({})",
                        x.old_name,
                        id_text(&x.old_id),
                        c.name,
                        c.id
                    ));
                    if let Some((from, to)) = &x.folder {
                        ui.label(widgets::caption(format!("   folder: {from}  →  {to}")));
                    }
                }
                if !refused.is_empty() {
                    ui.add_space(6.0);
                    ui.colored_label(
                        theme::error(),
                        format!(
                            "{} cannot be changed and will be left as they are:",
                            clients(refused.len())
                        ),
                    );
                    for (c, r) in &refused {
                        if let Err(e) = r {
                            ui.label(widgets::caption(format!("{} ({}): {e}", c.name, c.id)));
                        }
                    }
                }
            });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !ok.is_empty(),
                    egui::Button::new(format!("Yes, update {}", clients(ok.len()))),
                )
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
            // The ones that can be changed, in order (a refused one is not tried again).
            let changes: Vec<Change> = ok.iter().map(|(c, _)| c.clone()).collect();
            p.preview = None;
            p.progress = Default::default();
            p.of = changes.len();
            let pr = p.progress.clone();
            match on_thread(
                &ctx,
                env,
                catalog,
                "Updating the clients…",
                move |conn, root, dev| {
                    listupdate::apply(conn, root, dev, &changes, &pr.done, &pr.stop)
                },
            ) {
                Ok(j) => {
                    p.work = Some(j);
                    p.notice = None;
                    p.results.clear();
                }
                Err(e) => p.error = Some(e),
            }
        }
        Some(false) => p.preview = None,
        None => {}
    }
}

/// Whether Tick the ones shown ticks `m` with Show set to `group`. A wrong match renames a
/// folder and changes an ID, so only what is surely one client is ticked for the person,
/// and each less sure kind only when they have asked to see that kind by itself.
fn bulk_tickable(m: &Match, group: Group) -> bool {
    match (m.tier, m.kind) {
        (Tier::Near, _) => false,
        (_, Kind::OtherId) => group == Group::OtherId && m.tier == Tier::Exact,
        (Tier::Exact, _) => true,
        (Tier::Rule, _) => group == Group::Rule,
    }
}

fn kind_cell(ui: &mut egui::Ui, m: &Match) {
    let pct = format!("{:.0}%", (m.score * 100.0).floor());
    let (name, why) = match m.tier {
        Tier::Exact => (
            "Same name".to_string(),
            "The same name apart from capitals, punctuation and \"Last, First\".",
        ),
        Tier::Rule => (
            "Same, written differently".to_string(),
            "The same words once legal forms (LLC, Inc), \"the\", \"and\" and a middle initial on one side are set aside, or in another order.",
        ),
        Tier::Near => (
            format!("Close {pct}: look"),
            "Close, with a difference that can mean another client: a number, Jr or II on one side, two legal forms, a word more, a word shortened or a letter off.",
        ),
    };
    let (text, mark) = match m.kind {
        Kind::NoId => (name, m.tier == Tier::Near),
        Kind::OtherId => (format!("{name} · has another ID"), true),
        Kind::SameId => (
            format!("Same ID · {}", name.to_lowercase()),
            m.tier == Tier::Near,
        ),
    };
    let text = if mark {
        RichText::new(text).color(theme::warn())
    } else {
        RichText::new(text)
    };
    ui.label(text).on_hover_text(match m.kind {
        Kind::OtherId => format!("{why} The client has a different ID already: two people of one name are not one client, so look before you tick."),
        _ => why.to_string(),
    });
}

fn grid(ui: &mut egui::Ui, p: &mut ListUpdatePage) {
    let shown = p.shown();
    let mut toggled: Option<(String, bool)> = None;
    ui.visuals_mut().selection.stroke.color = theme::ink();
    let Some(f) = &p.found else {
        return;
    };
    egui::ScrollArea::horizontal()
        .id_salt("listupdate-table-x")
        .show(ui, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::exact(24.0))
                .column(
                    Column::initial(190.0)
                        .at_least(110.0)
                        .clip(true)
                        .resizable(true),
                )
                .column(
                    Column::initial(360.0)
                        .at_least(160.0)
                        .clip(true)
                        .resizable(true),
                )
                .column(
                    Column::initial(420.0)
                        .at_least(160.0)
                        .clip(true)
                        .resizable(true),
                )
                .header(28.0, |mut h| {
                    h.col(|_| {});
                    for name in ["Match", "On the list", "Client now"] {
                        h.col(|ui| {
                            ui.label(
                                RichText::new(name.to_uppercase())
                                    .small()
                                    .color(theme::ink_muted()),
                            );
                        });
                    }
                })
                .body(|body| {
                    body.rows(ROW_H, shown.len(), |mut row| {
                        let m = &f.matches[shown[row.index()]];
                        let mut sel = p.picked.contains(&m.client.id);
                        row.set_selected(sel);
                        row.col(|ui| {
                            let r = ui.add(egui::Checkbox::without_text(&mut sel));
                            let now = sel;
                            r.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::accesskit::Role::CheckBox,
                                    true,
                                    now,
                                    format!(
                                        "Update {} to {} {}",
                                        m.client.name, m.row.name, m.row.id
                                    ),
                                )
                            });
                            if r.changed() {
                                toggled = Some((m.client.id.clone(), sel));
                            }
                        });
                        row.col(|ui| kind_cell(ui, m));
                        row.col(|ui| {
                            ui.label(RichText::new(&m.row.name).color(theme::ink()));
                            ui.label(RichText::new(&m.row.id).family(theme::mono_semibold(ui)));
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&m.client.name).color(theme::ink()))
                                .on_hover_text(
                                    m.client
                                        .folder
                                        .as_deref()
                                        .unwrap_or("No folder in the cabinet."),
                                );
                            ui.label(
                                RichText::new(id_text(&m.client.ext_id))
                                    .family(theme::mono_semibold(ui)),
                            );
                            ui.label(widgets::caption(format!(
                                "{} document{}{}",
                                m.client.docs,
                                if m.client.docs == 1 { "" } else { "s" },
                                if m.client.folder.is_none() {
                                    " · no folder"
                                } else {
                                    ""
                                }
                            )));
                        });
                    });
                });
        });
    if let Some((k, on)) = toggled {
        if on {
            p.picked.insert(k);
        } else {
            p.picked.remove(&k);
        }
    }
}
