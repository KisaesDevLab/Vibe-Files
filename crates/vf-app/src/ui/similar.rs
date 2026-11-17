//! Admin › Similar clients (owner 2026-11-16, Q-129, PA-90): the same person or business made
//! twice in the client list. The page lists the pairs (same ID apart from its punctuation, or
//! names that match 97%), a person ticks the ones that are one client and says which is kept,
//! sees what each merge would do, and merges them together. Nothing is ticked for them.
//!
//! Finding compares every client with every other and each merge moves files, so the find,
//! the preview, the run and its undo are on a thread with their own catalog connection.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

use egui::{Align, Layout, RichText};
use egui_extras::{Column, TableBuilder};
use vf_core::db::Connection;
use vf_fs::clientmerge::Merged;
use vf_fs::similar::{self, Choice, Found, Outcome, Pair, Preview, Side, Why};

use crate::ui::browse::Env;
use crate::ui::job::{self, Job};
use crate::ui::theme;
use crate::ui::widgets::{self, UiExt as _};

const ROW_H: f32 = 26.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Group {
    #[default]
    All,
    SameId,
    SameName,
}

#[derive(Default)]
struct Progress {
    done: AtomicUsize,
    stop: AtomicBool,
}

type Key = (String, String);

/// What a job on the "not the same" marks did.
enum Marked {
    /// This pair was marked.
    Pair(Key),
    /// Every mark was forgotten.
    All,
}
type Previewed = Vec<(Choice, Result<Preview, String>)>;
type Ran = Vec<(Choice, Outcome)>;
/// For each merge put back: the client, and how many documents went back or why not.
type PutBack = Vec<(String, similar::PutBack)>;

#[derive(Default)]
pub struct SimilarPage {
    found: Option<Found>,
    finding: Option<Job<Result<Found, String>>>,
    filter: String,
    group: Group,
    /// Ticked pairs.
    picked: BTreeSet<Key>,
    /// Which one is kept where a person chose (true: the first).
    keep: HashMap<Key, bool>,
    previewing: Option<Job<Result<Result<Previewed, String>, String>>>,
    /// A "Not the same" mark being written, or every mark being forgotten (a file in the
    /// cabinet, under its lock: not on the window's thread).
    marking: Option<Job<Result<Marked, String>>>,
    /// What the ticked merges would do, shown for a yes or no.
    preview: Option<Previewed>,
    work: Option<Job<Result<Ran, String>>>,
    progress: Arc<Progress>,
    of: usize,
    /// The merges of the last run, for its undo.
    last_run: Vec<Merged>,
    undoing: Option<Job<Result<PutBack, String>>>,
    /// What the last run or undo did, pair by pair (the ones that need a look).
    results: Vec<String>,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The catalog changed: Browse re-reads.
    pub changed: bool,
    refind: bool,
    /// The pairs the last run took off the list (a client in them was merged away): where
    /// each was, the pair, and the merged client. Put back when that merge is undone. The
    /// clients are compared again only when the person presses Find (owner 2026-11-17: a
    /// compare after every merge greyed the list for as long as it took).
    set_aside: Vec<(usize, Pair, String)>,
}

fn key(p: &Pair) -> Key {
    (p.a.id.clone(), p.b.id.clone())
}

fn pairs(n: usize) -> String {
    format!("{n} pair{}", if n == 1 { "" } else { "s" })
}

fn docs(n: usize) -> String {
    format!("{n} document{}", if n == 1 { "" } else { "s" })
}

fn who(s: &Side) -> String {
    format!("{}  {}", s.name, s.ext_id)
}

/// The client's ID for a button, or its name when it has none.
fn id_or_name(s: &Side) -> &str {
    if s.ext_id.trim().is_empty() {
        &s.name
    } else {
        &s.ext_id
    }
}

impl SimilarPage {
    /// Whether the clients are being compared (tests).
    #[cfg(test)]
    pub fn is_finding(&self) -> bool {
        self.finding.is_some()
    }

    /// Which of the pair is kept (true: the first), when it is settled.
    fn keep_a(&self, p: &Pair) -> Option<bool> {
        self.keep.get(&key(p)).copied().or(p.keep_a)
    }

    /// Why the pair cannot be ticked yet.
    fn blocked(&self, p: &Pair) -> Option<String> {
        let keep_a = self.keep_a(p)?;
        let kept = if keep_a { &p.a } else { &p.b };
        kept.folder.is_none().then(|| {
            format!(
                "{} has no folder to merge into. Keep the other one, or create its folder in Browse first.",
                who(kept)
            )
        })
    }

    fn can_tick(&self, p: &Pair) -> bool {
        self.keep_a(p).is_some() && self.blocked(p).is_none()
    }

    /// The pairs the filter and the group show, by their place in the found list.
    fn shown(&self) -> Vec<usize> {
        let Some(f) = &self.found else {
            return vec![];
        };
        let q = self.filter.trim().to_lowercase();
        f.pairs
            .iter()
            .enumerate()
            .filter(|(_, p)| match self.group {
                Group::All => true,
                Group::SameId => p.why == Why::SameId,
                Group::SameName => p.why == Why::SameName,
            })
            .filter(|(_, p)| {
                q.is_empty()
                    || [&p.a, &p.b].iter().any(|s| {
                        s.name.to_lowercase().contains(&q) || s.ext_id.to_lowercase().contains(&q)
                    })
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The ticked pairs as merges, in the list's order.
    fn choices(&self) -> Vec<Choice> {
        let Some(f) = &self.found else {
            return vec![];
        };
        f.pairs
            .iter()
            .filter(|p| self.picked.contains(&key(p)))
            .filter_map(|p| {
                let (kept, gone) = if self.keep_a(p)? {
                    (&p.a, &p.b)
                } else {
                    (&p.b, &p.a)
                };
                Some(Choice {
                    from: gone.id.clone(),
                    into: kept.id.clone(),
                })
            })
            .collect()
    }

    fn find(&mut self, ctx: &egui::Context, env: &Env, catalog: &Path) {
        let (catalog, root) = (catalog.to_path_buf(), env.root.to_path_buf());
        match Job::start(
            ctx,
            "Comparing every client with every other…",
            move || {
                let conn = vf_core::db::open(&catalog, vf_core::db::Storage::Local)
                    .map_err(|e| e.to_string())?;
                similar::find(&conn, &root)
            },
        ) {
            Ok(j) => {
                self.finding = Some(j);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Takes the pairs with a merged-away client off the list, remembered for the undo.
    fn set_aside(&mut self, gone: &BTreeSet<String>) {
        let Some(f) = &mut self.found else {
            return;
        };
        let mut kept = Vec::with_capacity(f.pairs.len());
        for (i, x) in std::mem::take(&mut f.pairs).into_iter().enumerate() {
            let by = [&x.a.id, &x.b.id].into_iter().find(|id| gone.contains(*id));
            match by {
                Some(id) => {
                    let id = id.clone();
                    self.picked.remove(&key(&x));
                    self.keep.remove(&key(&x));
                    self.set_aside.push((i, x, id));
                }
                None => kept.push(x),
            }
        }
        f.pairs = kept;
        f.clients = f.clients.saturating_sub(gone.len());
    }

    /// Puts back the pairs set aside for the merges that were undone.
    fn bring_back(&mut self, back: &BTreeSet<String>) {
        let Some(f) = &mut self.found else {
            return;
        };
        let (mut again, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.set_aside)
            .into_iter()
            .partition(|(_, _, id)| back.contains(id));
        self.set_aside = rest;
        again.sort_by_key(|(i, _, _)| *i);
        for (i, x, _) in again {
            let at = i.min(f.pairs.len());
            f.pairs.insert(at, x);
        }
        f.clients += back.len();
    }

    fn took(&mut self, f: Found) {
        self.set_aside.clear();
        let live: BTreeSet<Key> = f.pairs.iter().map(key).collect();
        self.picked.retain(|k| live.contains(k));
        self.keep.retain(|k, _| live.contains(k));
        self.found = Some(f);
        // A tick stands only while the pair can still be merged as chosen (the CSV may have
        // changed which one is kept, or the kept one lost its folder).
        let still: BTreeSet<Key> = self
            .found
            .iter()
            .flat_map(|f| &f.pairs)
            .filter(|p| self.can_tick(p))
            .map(key)
            .collect();
        self.picked.retain(|k| still.contains(k));
    }

    /// Takes a finished run or undo, whichever tab is shown (the app calls this each frame):
    /// Browse must read the catalog again.
    pub fn poll(&mut self) {
        if let Some(r) = job::take(&mut self.work) {
            match r.and_then(|r| r) {
                Ok(out) => self.ran(out),
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = job::take(&mut self.undoing) {
            match r.and_then(|r| r) {
                Ok(back) => {
                    // Put back from the end of the run; it stops at the first that cannot be.
                    let ok = back.iter().filter(|(_, r)| r.is_ok()).count();
                    self.results = back
                        .iter()
                        .filter_map(|(c, r)| match r {
                            Err(e) => Some(format!("{c}: {e}")),
                            // Put back, but not all of the client's own: said too.
                            Ok((_, Some(note))) => Some(format!("{c}: {note}")),
                            Ok((_, None)) => None,
                        })
                        .collect();
                    let left = self.last_run.len().saturating_sub(ok);
                    // The clients that are their own again come back into the list.
                    let back: BTreeSet<String> = self.last_run[left..]
                        .iter()
                        .map(|m| m.from.id.clone())
                        .collect();
                    self.bring_back(&back);
                    self.last_run.truncate(left);
                    self.notice = Some(format!(
                        "{} put back.{}",
                        if ok == 1 {
                            "1 merge".to_string()
                        } else {
                            format!("{ok} merges")
                        },
                        if left == 0 {
                            String::new()
                        } else {
                            format!(
                                " Stopped there: {left} of the run {} still merged (the reason is below). Put right what it says, then press Undo again.",
                                if left == 1 { "is" } else { "are" }
                            )
                        }
                    ));
                    self.changed = true;
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    fn ran(&mut self, out: Vec<(Choice, Outcome)>) {
        let names: HashMap<&str, String> = self
            .found
            .iter()
            .flat_map(|f| &f.pairs)
            .flat_map(|p| [(p.a.id.as_str(), who(&p.a)), (p.b.id.as_str(), who(&p.b))])
            .collect();
        let name = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_string());
        let (mut merged, mut moved) = (vec![], 0);
        let mut gone: BTreeSet<String> = BTreeSet::new();
        self.results.clear();
        for (c, o) in out {
            match o {
                Outcome::Merged(m) => {
                    moved += m
                        .moves
                        .iter()
                        .filter(|x| x.from != x.to && !x.is_notes())
                        .count();
                    if let Some(n) = &m.note {
                        self.results
                            .push(format!("{} into {}: {n}", name(&c.from), name(&c.into)));
                    }
                    gone.insert(c.from.clone());
                    merged.push(*m);
                }
                Outcome::Skipped(why) => self.results.push(format!(
                    "{} into {}: not merged. {why}",
                    name(&c.from),
                    name(&c.into)
                )),
                Outcome::Failed(why) => self.results.push(format!(
                    "{} into {}: refused. {why}",
                    name(&c.from),
                    name(&c.into)
                )),
            }
        }
        drop(names);
        self.notice = Some(format!(
            "Merged {}: {} moved. The list is not compared again until you press Find similar clients.{}",
            pairs(merged.len()),
            docs(moved),
            if self.results.is_empty() {
                ""
            } else {
                " These need a look:"
            }
        ));
        self.error = None;
        // A run that merged nothing leaves the last run's Undo as it was.
        if !merged.is_empty() {
            self.last_run = merged;
            // Only the last run can be undone: what earlier runs set aside stays off.
            self.set_aside.clear();
            // The merged clients' rows leave the list; nothing is compared again.
            self.set_aside(&gone);
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

pub fn show(ui: &mut egui::Ui, p: &mut SimilarPage, env: &mut Env, catalog: &Path) {
    let ctx = ui.ctx().clone();
    if let Some(r) = job::take(&mut p.finding) {
        match r.and_then(|r| r) {
            Ok(f) => p.took(f),
            Err(e) => p.error = Some(format!("The clients could not be compared: {e}")),
        }
    }
    if let Some(r) = job::take(&mut p.previewing) {
        match r.and_then(|r| r).and_then(|r| r) {
            Ok(pv) => p.preview = Some(pv),
            Err(e) => p.error = Some(e),
        }
    }
    if let Some(r) = job::take(&mut p.marking) {
        match r.and_then(|r| r) {
            Ok(Marked::Pair(k)) => {
                p.picked.remove(&k);
                p.keep.remove(&k);
                if let Some(f) = &mut p.found {
                    f.pairs.retain(|x| key(x) != k);
                    f.dismissed += 1;
                }
            }
            Ok(Marked::All) => p.refind = true,
            Err(e) => p.error = Some(e),
        }
    }
    p.poll();
    if std::mem::take(&mut p.refind) {
        p.find(&ctx, env, catalog);
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Similar clients").strong().size(16.0));
        ui.label(widgets::caption(
            "Clients made twice: the same ID written two ways, or names that match 97%. Tick the pairs that are one client and merge them.",
        ));
    });
    ui.add_space(6.0);
    let idle = p.finding.is_none()
        && p.marking.is_none()
        && p.previewing.is_none()
        && p.work.is_none()
        && p.undoing.is_none();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(idle, egui::Button::new("Find similar clients"))
            .on_hover_text(
                "Compares every client in the catalog with every other. Nothing is changed.",
            )
            .clicked()
        {
            p.notice = None;
            p.results.clear();
            p.find(&ctx, env, catalog);
        }
        for j in [p.finding.as_ref().map(|j| j as &dyn Shown)]
            .into_iter()
            .chain([p.previewing.as_ref().map(|j| j as &dyn Shown)])
            .chain([p.undoing.as_ref().map(|j| j as &dyn Shown)])
            .chain([p.marking.as_ref().map(|j| j as &dyn Shown)])
            .flatten()
        {
            j.draw(ui);
        }
    });
    if let Some(j) = &p.work {
        ui.horizontal(|ui| {
            j.show(ui);
            ui.label(format!("{} of {}", p.progress.done.load(Relaxed), p.of));
            let stopping = p.progress.stop.load(Relaxed);
            if ui
                .add_enabled(!stopping, egui::Button::new("Cancel"))
                .on_hover_text("Stops after the pair being merged now. What is merged stays merged; Undo these merges takes it back.")
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
            .id_salt("similar-results")
            .max_height(110.0)
            .show(ui, |ui| {
                for r in &p.results {
                    ui.label(widgets::caption(r));
                }
            });
    }
    if !p.last_run.is_empty() {
        let n = p.last_run.len();
        if ui
            .add_enabled(
                idle,
                egui::Button::new(format!(
                    "Undo {}",
                    if n == 1 {
                        "this merge".to_string()
                    } else {
                        format!("these {n} merges")
                    }
                )),
            )
            .on_hover_text("Puts the merged clients back, the last one first: their folders, documents, place on the client list and in the CSV.")
            .clicked()
        {
            let merged = p.last_run.clone();
            match on_thread(&ctx, env, catalog, "Putting the merges back…", move |conn, root, dev| {
                similar::undo_many(conn, root, dev, &merged)
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
    if let Some(n) = &f.csv_note {
        ui.label(widgets::caption(n));
    }
    let (total, more, compared, dismissed) = (f.pairs.len(), f.more, f.clients, f.dismissed);
    ui.horizontal_wrapped(|ui| {
        ui.label(if total == 0 {
            format!("No similar clients among {compared}.")
        } else if more {
            format!(
                "First {} shown of more; merge or dismiss some, then find again. {} ticked.",
                pairs(total),
                p.picked.len()
            )
        } else {
            format!(
                "{} among {compared} clients · {} ticked",
                pairs(total),
                p.picked.len()
            )
        });
        if dismissed > 0
            && ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("Show the {dismissed} marked not the same")).small(),
                )
                .on_hover_text("Forgets every \"Not the same\" mark, on every PC, and finds again.")
                .clicked()
        {
            let root = env.root.to_path_buf();
            match Job::start(&ctx, "Forgetting the marks…", move || {
                similar::forget_dismissed(&root).map(|_| Marked::All)
            }) {
                Ok(j) => p.marking = Some(j),
                Err(e) => p.error = Some(e),
            }
        }
    });
    if total == 0 {
        return;
    }
    ui.horizontal(|ui| {
        let l = ui.label("Show");
        egui::ComboBox::from_id_salt("similar-group")
            .selected_text(match p.group {
                Group::All => "All pairs",
                Group::SameId => "Same ID",
                Group::SameName => "Same name, different IDs",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut p.group, Group::All, "All pairs");
                ui.selectable_value(&mut p.group, Group::SameId, "Same ID");
                ui.selectable_value(&mut p.group, Group::SameName, "Same name, different IDs");
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
            .on_hover_text("Ticks every pair shown that has a client to keep. Pairs where you must still choose which one is kept are left.")
            .clicked()
            && let Some(f) = &p.found
        {
            let add: Vec<Key> = shown
                .iter()
                .map(|&i| &f.pairs[i])
                .filter(|x| p.can_tick(x))
                .map(key)
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
            egui::Button::new(format!("Merge the {} ticked…", pairs(n))),
        )
        .on_hover_text(
            "Shows what each merge would do first. Nothing is changed until you say yes.",
        )
        .on_disabled_hover_text("Tick the pairs that are one client.")
        .clicked()
    {
        let choices = p.choices();
        match on_thread(
            &ctx,
            env,
            catalog,
            "Working out the merges…",
            move |conn, root, _| similar::preview(conn, root, &choices),
        ) {
            Ok(j) => {
                p.previewing = Some(j);
                p.error = None;
            }
            Err(e) => p.error = Some(e),
        }
    }
    confirm(ui, p, env, catalog);
    let to_choose = p
        .found
        .iter()
        .flat_map(|f| &f.pairs)
        .filter(|x| p.keep_a(x).is_none())
        .count();
    if to_choose > 0 {
        ui.label(widgets::caption(format!(
            "{} cannot be ticked yet: neither or both clients are in the client CSV, so press Keep on the one that stays first.",
            pairs(to_choose)
        )));
    }
    ui.add_space(6.0);
    // Not while a run, an undo or a mark is being made: a tick made then would be wiped
    // when it ends, and a mark would wait on the cabinet's lock in front of the window.
    ui.add_enabled_ui(idle, |ui| grid(ui, p, env));
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

/// What the ticked merges would do, for a yes or no.
fn confirm(ui: &mut egui::Ui, p: &mut SimilarPage, env: &mut Env, catalog: &Path) {
    let Some(pv) = &p.preview else {
        return;
    };
    let ctx = ui.ctx().clone();
    let ok: Vec<&Preview> = pv.iter().filter_map(|(_, r)| r.as_ref().ok()).collect();
    let refused: Vec<(&Choice, &String)> = pv
        .iter()
        .filter_map(|(c, r)| r.as_ref().err().map(|e| (c, e)))
        .collect();
    let moves: usize = ok.iter().map(|x| x.moves).sum();
    let both: usize = ok.iter().map(|x| x.kept_both).sum();
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("similar-merge")).show(&ctx, |ui| {
        ui.set_width(620.0);
        ui.heading(format!("Merge {}?", pairs(ok.len())));
        ui.label(format!(
            "{} move to the kept clients' folders.{} The merged clients leave the client list and the client CSV; their relationships and legal holds become the kept clients'.",
            docs(moves),
            if both > 0 {
                format!(
                    " {both} have a name already taken there: both are kept, the one moved with \" (2)\"."
                )
            } else {
                String::new()
            }
        ));
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("similar-preview")
            .max_height(260.0)
            .show(ui, |ui| {
                for x in &ok {
                    ui.label(format!(
                        "{}  →  {}   ·   {}{}{}",
                        x.from,
                        x.into,
                        docs(x.moves),
                        if x.kept_both > 0 {
                            format!(", {} kept twice", x.kept_both)
                        } else {
                            String::new()
                        },
                        if x.after_another {
                            "  (after a merge above: counted as things are now)"
                        } else {
                            ""
                        }
                    ));
                    if x.drops_relationships {
                        ui.label(widgets::caption(format!(
                            "   {} has no ID, so the relationships of {} cannot become its own: they are dropped.",
                            x.into.trim(),
                            x.from.trim()
                        )));
                    }
                }
                if !refused.is_empty() {
                    ui.add_space(6.0);
                    ui.colored_label(
                        theme::error(),
                        format!("{} cannot be merged and will be left as they are:", pairs(refused.len())),
                    );
                    for (_, e) in &refused {
                        ui.label(widgets::caption(e.as_str()));
                    }
                }
            });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !ok.is_empty(),
                    egui::Button::new(format!("Yes, merge {}", pairs(ok.len()))),
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
            // The ones that can be merged, in order (a refused pair is not tried again).
            let choices: Vec<Choice> = pv
                .iter()
                .filter(|(_, r)| r.is_ok())
                .map(|(c, _)| c.clone())
                .collect();
            p.preview = None;
            p.progress = Default::default();
            p.of = choices.len();
            let pr = p.progress.clone();
            match on_thread(
                &ctx,
                env,
                catalog,
                "Merging the clients…",
                move |conn, root, dev| {
                    similar::merge_many(conn, root, dev, &choices, &pr.done, &pr.stop)
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

fn side_cell(ui: &mut egui::Ui, s: &Side) {
    ui.label(RichText::new(&s.name).color(theme::ink()))
        .on_hover_text(s.folder.as_deref().unwrap_or("No folder in the cabinet."));
    ui.label(RichText::new(&s.ext_id).family(theme::mono_semibold(ui)));
    ui.label(widgets::caption(format!(
        "{}{}{}",
        docs(s.docs),
        if s.in_csv { " · in the CSV" } else { "" },
        if s.folder.is_none() {
            " · no folder"
        } else {
            ""
        }
    )));
}

fn grid(ui: &mut egui::Ui, p: &mut SimilarPage, env: &mut Env) {
    let shown = p.shown();
    let mut toggled: Option<(Key, bool)> = None;
    let mut keep: Option<(Key, bool)> = None;
    let mut not_same: Option<Key> = None;
    ui.visuals_mut().selection.stroke.color = theme::ink();
    let Some(f) = &p.found else {
        return;
    };
    egui::ScrollArea::horizontal()
        .id_salt("similar-table-x")
        .show(ui, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::exact(24.0))
                .column(Column::initial(96.0).at_least(70.0).clip(true).resizable(true))
                .column(Column::initial(330.0).at_least(140.0).clip(true).resizable(true))
                .column(Column::initial(330.0).at_least(140.0).clip(true).resizable(true))
                .column(Column::initial(210.0).at_least(120.0).clip(true).resizable(true))
                .column(Column::exact(110.0))
                .header(28.0, |mut h| {
                    h.col(|_| {});
                    for name in ["Match", "Client", "Similar client", "Keep", ""] {
                        h.col(|ui| {
                            ui.label(RichText::new(name.to_uppercase()).small().color(theme::ink_muted()));
                        });
                    }
                })
                .body(|body| {
                    body.rows(ROW_H, shown.len(), |mut row| {
                        let x = &f.pairs[shown[row.index()]];
                        let k = key(x);
                        let mut sel = p.picked.contains(&k);
                        let keep_a = p.keep_a(x);
                        row.set_selected(sel);
                        row.col(|ui| {
                            let why_not = match keep_a {
                                None => Some("Choose which one is kept first.".to_string()),
                                Some(_) => p.blocked(x),
                            };
                            let r = ui
                                .add_enabled(
                                    why_not.is_none() || sel,
                                    egui::Checkbox::without_text(&mut sel),
                                )
                                .on_disabled_hover_text(why_not.unwrap_or_default());
                            let now = sel;
                            r.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::accesskit::Role::CheckBox,
                                    true,
                                    now,
                                    format!("Merge {} and {}", who(&x.a), who(&x.b)),
                                )
                            });
                            if r.changed() {
                                toggled = Some((k.clone(), sel));
                            }
                        });
                        row.col(|ui| {
                            let pct = format!("{:.0}%", (x.name_score * 100.0).floor());
                            match x.why {
                                Why::SameId => ui
                                    .label("Same ID")
                                    .on_hover_text(format!("The IDs are the same apart from capitals, spaces, dashes, underscores and leading zeros. Names match {pct}.")),
                                Why::SameName => ui
                                    .label(format!("Name {pct}"))
                                    .on_hover_text("The names match; the IDs are different. Two people of one name, or two businesses of one family, are not one client: look before you tick."),
                            };
                        });
                        row.col(|ui| side_cell(ui, &x.a));
                        row.col(|ui| side_cell(ui, &x.b));
                        row.col(|ui| {
                            for (side, is_a) in [(&x.a, true), (&x.b, false)] {
                                let r = ui
                                    .selectable_label(
                                        keep_a == Some(is_a),
                                        format!("Keep {}", id_or_name(side)),
                                    )
                                    .on_hover_text(format!(
                                        "{} stays; the other client's documents move into its folder.",
                                        who(side)
                                    ));
                                r.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::accesskit::Role::RadioButton,
                                        true,
                                        keep_a == Some(is_a),
                                        format!(
                                            "Keep {} of {} and {}",
                                            id_or_name(side),
                                            who(&x.a),
                                            who(&x.b)
                                        ),
                                    )
                                });
                                if r.clicked() {
                                    keep = Some((k.clone(), is_a));
                                }
                            }
                        });
                        row.col(|ui| {
                            let r = ui
                                .small_btn("Not the same")
                                .on_hover_text("These are two clients. The pair is not listed again, on any PC.");
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::Role::Button,
                                    true,
                                    format!("Not the same: {} and {}", who(&x.a), who(&x.b)),
                                )
                            });
                            if r.clicked() {
                                not_same = Some(k.clone());
                            }
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
    if let Some((k, a)) = keep {
        p.keep.insert(k.clone(), a);
        // A tick stands only while the kept one has a folder.
        if let Some(x) = p
            .found
            .as_ref()
            .and_then(|f| f.pairs.iter().find(|x| key(x) == k))
            && !p.can_tick(x)
        {
            p.picked.remove(&k);
        }
    }
    if let Some(k) = not_same {
        let root = env.root.to_path_buf();
        match Job::start(ui.ctx(), "Marking the pair…", move || {
            similar::dismiss(&root, &k.0, &k.1).map(|()| Marked::Pair(k))
        }) {
            Ok(j) => p.marking = Some(j),
            Err(e) => p.error = Some(e),
        }
    }
}
