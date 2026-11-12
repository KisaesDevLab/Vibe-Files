//! Admin › Integrity sweep (PHASES P05 step 5). The sweep re-hashes the client folders on a
//! background thread (its own catalog connection): when the cabinet opens if the last one is a
//! week old, and on "Sweep now". Files edited outside Vibe Files are adopted as soon as the
//! result arrives (Q-29); moves, missing and unknown files wait for a click here.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::RichText;
use vf_core::db::{Connection, Storage};
use vf_fs::sweep::{self, Drift};

use crate::ui::browse::Env;
use crate::ui::{theme, widgets};

/// Another session on this computer is sweeping this catalog.
const BUSY: &str =
    "Another session on this computer is running the integrity sweep; its result shows there.";

/// Held while a sweep runs on `catalog` (a file beside it, no sharing).
fn sweep_lock(catalog: &Path) -> Option<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = catalog.parent()?;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(dir.join("sweep.lock"))
        .ok()
}

/// Which list a finished action clears from the result shown.
enum Act {
    Moves,
    Missing,
    /// Unknown files adopted or sent to Intake (their cabinet paths).
    Unknown(BTreeSet<String>),
}

/// An action's result: files done (or why not), what it did, and the list it clears.
type ActDone = (Result<usize, String>, &'static str, Act);

#[derive(Default)]
pub struct SweepState {
    job: Option<Receiver<Result<Drift, String>>>,
    /// Follow moves / Mark deleted / Adopt / Send to Intake running on its thread (they hash
    /// or move every file listed; owner 2026-11-12: a spinner, not a frozen window).
    act: Option<crate::ui::job::Job<ActDone>>,
    /// The last result, after adopting the modified files.
    pub drift: Option<Drift>,
    /// When it finished (RFC 3339).
    pub when: Option<String>,
    /// Files adopted on their own by the last sweep.
    pub adopted: Vec<String>,
    /// Ticked unknown files (cabinet paths).
    picked: BTreeSet<String>,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The catalog changed: Browse re-reads.
    pub changed: bool,
}

impl SweepState {
    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    /// Starts a sweep on its own thread.
    pub fn start(&mut self, ctx: &egui::Context, catalog: &Path, root: &Path) {
        if self.running() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (catalog, root, ctx) = (catalog.to_path_buf(), root.to_path_buf(), ctx.clone());
        std::thread::Builder::new()
            .name("integrity-sweep".into())
            .spawn(move || {
                // One sweep per catalog at a time: on a server, every login's Vibe Files would
                // start the weekly one together (PA-34).
                let r = match sweep_lock(&catalog) {
                    Some(_held) => vf_core::db::open(&catalog, Storage::Local)
                        .map_err(|e| e.to_string())
                        .and_then(|conn| sweep::scan(&conn, &root)),
                    None => Err(BUSY.to_string()),
                };
                let _ = tx.send(r);
                ctx.request_repaint();
            })
            .ok();
        self.job = Some(rx);
    }

    /// Takes a finished sweep: adopts the modified files and remembers the rest. Returns a
    /// line for Browse when something needs attention.
    pub fn poll(&mut self, conn: &mut Connection, root: &Path, device_id: &str) -> Option<String> {
        let rx = self.job.as_ref()?;
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("the sweep stopped".into()),
        };
        self.job = None;
        let mut d = match r {
            Ok(d) => d,
            Err(e) if e == BUSY => {
                self.notice = Some(BUSY.into());
                return None;
            }
            Err(e) => {
                tracing::warn!(kind = crate::logging::kind(&e), "integrity sweep failed");
                self.error = Some(format!("Integrity sweep failed: {e}"));
                return None;
            }
        };
        let now = vf_core::now_rfc3339();
        let _ = sweep::set_last_sweep(conn, &now);
        self.when = Some(now);
        self.adopted.clear();
        if !d.modified.is_empty() {
            match sweep::adopt_modified(conn, root, device_id, &d.modified) {
                Ok(adopted) => {
                    self.adopted = adopted;
                    d.modified.clear();
                    self.changed = true;
                }
                Err(e) => self.error = Some(format!("Adopting edited files: {e}")),
            }
        }
        self.picked.clear();
        tracing::info!(
            checked = d.checked,
            adopted = self.adopted.len(),
            moved = d.moved.len(),
            missing = d.missing.len(),
            unknown = d.unknown.len(),
            unreadable = d.unreadable.len(),
            "integrity sweep"
        );
        let open = d.moved.len() + d.missing.len() + d.unknown.len();
        let line = match (self.adopted.len(), open) {
            (0, 0) => None,
            (a, 0) => Some(format!(
                "Integrity sweep: {a} files edited outside Vibe Files were kept as new versions."
            )),
            (a, n) => Some(format!(
                "Integrity sweep: {}{n} files need a look in Admin › Integrity sweep.",
                if a > 0 {
                    format!("{a} files edited outside Vibe Files were kept as new versions; ")
                } else {
                    String::new()
                }
            )),
        };
        self.drift = Some(d);
        line
    }
}

/// Starts an action on its own thread with its own catalog connection.
fn act(
    s: &mut SweepState,
    ctx: &egui::Context,
    catalog: &Path,
    running: String,
    what: &'static str,
    clears: Act,
    work: impl FnOnce(&mut Connection) -> Result<usize, String> + Send + 'static,
) {
    let catalog = catalog.to_path_buf();
    match crate::ui::job::Job::start(ctx, running, move || {
        let r = vf_core::db::open(&catalog, Storage::Local)
            .map_err(|e| e.to_string())
            .and_then(|mut conn| work(&mut conn));
        (r, what, clears)
    }) {
        Ok(j) => {
            s.error = None;
            s.notice = None;
            s.act = Some(j);
        }
        Err(e) => s.error = Some(e),
    }
}

/// Takes a finished action and applies it to the page.
fn poll_act(s: &mut SweepState) {
    let Some(r) = crate::ui::job::take(&mut s.act) else {
        return;
    };
    match r {
        Ok((r, what, clears)) => done(s, r, what, |d| match clears {
            Act::Moves => d.moved.clear(),
            Act::Missing => d.missing.clear(),
            Act::Unknown(paths) => d.unknown.retain(|u| !paths.contains(&u.rel_path)),
        }),
        Err(e) => s.error = Some(e),
    }
}

/// Filer's first inbox (where "Send to Intake" puts a file).
fn first_inbox(root: &Path) -> Option<PathBuf> {
    let (s, _) = filer_core::config::Settings::load_or_default(&vf_fs::clients::master_dir(root));
    s.inboxes.first().cloned()
}

pub fn show(ui: &mut egui::Ui, s: &mut SweepState, env: &mut Env, catalog: &Path) {
    poll_act(s);
    let idle = s.act.is_none() && !s.running();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Integrity sweep").strong().size(16.0));
        ui.label(widgets::caption(format!(
            "Re-hashes the client folders against the catalog; runs on its own every {} days.",
            sweep::EVERY_DAYS
        )));
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let last = s
            .when
            .clone()
            .or_else(|| sweep::last_sweep(env.conn))
            .map(|t| format!("Last sweep {}.", widgets::local_time(&t)))
            .unwrap_or_else(|| "No sweep yet.".into());
        ui.label(last);
        if s.running() {
            ui.spinner();
            ui.label("Sweeping…");
        } else if ui
            .add_enabled(s.act.is_none(), egui::Button::new("Sweep now"))
            .clicked()
        {
            s.start(ui.ctx(), catalog, env.root);
        }
    });
    if let Some(j) = &s.act {
        j.show(ui);
    }
    if let Some(n) = &s.notice {
        ui.label(RichText::new(n).color(theme::ink_secondary()));
    }
    if let Some(e) = &s.error {
        ui.label(RichText::new(e).color(crate::ui::theme::error()));
    }
    ui.separator();
    let Some(d) = s.drift.clone() else {
        return;
    };
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.label(format!("{} files checked.", d.checked));
            if d.online_only > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} files not checked: they are online only (OneDrive or Dropbox) and would be downloaded. They are checked once they are on this PC.",
                        d.online_only
                    ))
                    .color(theme::ink_secondary()),
                );
            }
            if !s.adopted.is_empty() {
                section(
                    ui,
                    &format!(
                        "Edited outside Vibe Files, kept as new versions ({})",
                        s.adopted.len()
                    ),
                );
                for p in &s.adopted {
                    ui.label(RichText::new(p).monospace().small());
                }
            }
            if d.is_clean() && d.unreadable.is_empty() && d.online_only == 0 {
                ui.label(
                    RichText::new("Everything matches the catalog.").color(theme::ink_secondary()),
                );
            }
            if !d.moved.is_empty() {
                section(ui, &format!("Moved outside Vibe Files ({})", d.moved.len()));
                for (m, u) in &d.moved {
                    ui.label(
                        RichText::new(format!("{}  →  {}", m.rel_path, u.rel_path))
                            .monospace()
                            .small(),
                    );
                }
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::new(format!("Follow {} moves", d.moved.len())),
                    )
                    .clicked()
                {
                    let (root, device, moved) =
                        (env.root.to_path_buf(), env.device_id.to_string(), d.moved.clone());
                    act(
                        s,
                        ui.ctx(),
                        catalog,
                        format!("Following {} moves (each file is checked)…", moved.len()),
                        "moves followed",
                        Act::Moves,
                        move |conn| sweep::follow_moves(conn, &root, &device, &moved),
                    );
                }
            }
            if !d.missing.is_empty() {
                section(ui, &format!("Missing ({})", d.missing.len()));
                for m in &d.missing {
                    ui.label(RichText::new(&m.rel_path).monospace().small());
                }
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::new(format!("Mark {} as deleted", d.missing.len())),
                    )
                    .on_hover_text("Their rows stay as tombstones; Undo takes it back.")
                    .clicked()
                {
                    let (root, device, missing) = (
                        env.root.to_path_buf(),
                        env.device_id.to_string(),
                        d.missing.clone(),
                    );
                    act(
                        s,
                        ui.ctx(),
                        catalog,
                        format!("Marking {} files as deleted…", missing.len()),
                        "marked deleted",
                        Act::Missing,
                        move |conn| sweep::mark_missing(conn, &root, &device, &missing),
                    );
                }
            }
            if !d.unknown.is_empty() {
                section(ui, &format!("Not in the catalog ({})", d.unknown.len()));
                for u in &d.unknown {
                    let mut on = s.picked.contains(&u.rel_path);
                    let name = RichText::new(&u.rel_path).monospace().small();
                    if ui.checkbox(&mut on, name).changed() {
                        if on {
                            s.picked.insert(u.rel_path.clone());
                        } else {
                            s.picked.remove(&u.rel_path);
                        }
                    }
                }
                let chosen: Vec<_> = d
                    .unknown
                    .iter()
                    .filter(|u| s.picked.contains(&u.rel_path))
                    .cloned()
                    .collect();
                ui.horizontal(|ui| {
                    let n = chosen.len();
                    if ui
                        .add_enabled(
                            n > 0 && idle,
                            egui::Button::new(format!("Adopt {n} where they are")),
                        )
                        .on_hover_text(
                            "They become documents in place; set their doc type in Browse.",
                        )
                        .clicked()
                    {
                        let (root, device, files) = (
                            env.root.to_path_buf(),
                            env.device_id.to_string(),
                            chosen.clone(),
                        );
                        act(
                            s,
                            ui.ctx(),
                            catalog,
                            format!("Adopting {n} files (each file is read)…"),
                            "adopted",
                            Act::Unknown(s.picked.clone()),
                            move |conn| sweep::adopt_unknown(conn, &root, &device, &files),
                        );
                    }
                    let inbox = first_inbox(env.root);
                    let send = ui
                        .add_enabled(
                            n > 0 && idle && inbox.is_some(),
                            egui::Button::new(format!("Send {n} to Intake")),
                        )
                        .on_disabled_hover_text(
                            "Intake needs an inbox folder (Intake › Settings).",
                        );
                    if send.clicked()
                        && let Some(inbox) = inbox
                    {
                        let (root, device, files) = (
                            env.root.to_path_buf(),
                            env.device_id.to_string(),
                            chosen.clone(),
                        );
                        act(
                            s,
                            ui.ctx(),
                            catalog,
                            format!("Sending {n} files to Intake's inbox…"),
                            "sent to Intake's inbox",
                            Act::Unknown(s.picked.clone()),
                            move |conn| {
                                sweep::refile_unknown(conn, &root, &inbox, &device, &files)
                                    .map(|v| v.len())
                            },
                        );
                    }
                });
            }
            if !d.unreadable.is_empty() {
                section(ui, &format!("Could not be read ({})", d.unreadable.len()));
                for u in &d.unreadable {
                    ui.label(RichText::new(u).small().color(crate::ui::theme::error()));
                }
            }
        });
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(title).strong());
}

/// Applies an action's result to the page.
fn done(s: &mut SweepState, r: Result<usize, String>, what: &str, drop: impl FnOnce(&mut Drift)) {
    match r {
        Ok(n) => {
            s.notice = Some(format!("{n} files {what}."));
            s.error = None;
            s.changed = true;
            s.picked.clear();
            if let Some(d) = s.drift.as_mut() {
                drop(d);
            }
        }
        Err(e) => s.error = Some(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One sweep per catalog at a time (several logins of a server share one).
    #[test]
    fn one_sweep_at_a_time() {
        let t = tempfile::tempdir().unwrap();
        let catalog = t.path().join("catalog.db");
        let first = sweep_lock(&catalog);
        assert!(first.is_some());
        assert!(
            sweep_lock(&catalog).is_none(),
            "another session is sweeping"
        );
        drop(first);
        assert!(sweep_lock(&catalog).is_some());
    }
}
