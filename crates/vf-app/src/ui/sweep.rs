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
use crate::ui::{theme, widgets, wizard::ERROR};

#[derive(Default)]
pub struct SweepState {
    job: Option<Receiver<Result<Drift, String>>>,
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
                let r = vf_core::db::open(&catalog, Storage::Local)
                    .map_err(|e| e.to_string())
                    .and_then(|conn| sweep::scan(&conn, &root));
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

/// Filer's first inbox (where "Send to Intake" puts a file).
fn first_inbox(root: &Path) -> Option<PathBuf> {
    let (s, _) = filer_core::config::Settings::load_or_default(&vf_fs::clients::master_dir(root));
    s.inboxes.first().cloned()
}

pub fn show(ui: &mut egui::Ui, s: &mut SweepState, env: &mut Env, catalog: &Path) {
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
        } else if ui.button("Sweep now").clicked() {
            s.start(ui.ctx(), catalog, env.root);
        }
    });
    if let Some(n) = &s.notice {
        ui.label(RichText::new(n).color(theme::INK_SECONDARY));
    }
    if let Some(e) = &s.error {
        ui.label(RichText::new(e).color(ERROR));
    }
    ui.separator();
    let Some(d) = s.drift.clone() else {
        return;
    };
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.label(format!("{} files checked.", d.checked));
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
            if d.is_clean() && d.unreadable.is_empty() {
                ui.label(
                    RichText::new("Everything matches the catalog.").color(theme::INK_SECONDARY),
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
                    .button(format!("Follow {} moves", d.moved.len()))
                    .clicked()
                {
                    let r = sweep::follow_moves(env.conn, env.root, env.device_id, &d.moved);
                    done(s, r, "moves followed", |d| d.moved.clear());
                }
            }
            if !d.missing.is_empty() {
                section(ui, &format!("Missing ({})", d.missing.len()));
                for m in &d.missing {
                    ui.label(RichText::new(&m.rel_path).monospace().small());
                }
                if ui
                    .button(format!("Mark {} as deleted", d.missing.len()))
                    .on_hover_text("Their rows stay as tombstones; Undo takes it back.")
                    .clicked()
                {
                    let r = sweep::mark_missing(env.conn, env.root, env.device_id, &d.missing);
                    done(s, r, "marked deleted", |d| d.missing.clear());
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
                            n > 0,
                            egui::Button::new(format!("Adopt {n} where they are")),
                        )
                        .on_hover_text(
                            "They become documents in place; set their doc type in Browse.",
                        )
                        .clicked()
                    {
                        let r = sweep::adopt_unknown(env.conn, env.root, env.device_id, &chosen);
                        let picked = s.picked.clone();
                        done(s, r, "adopted", |d| {
                            d.unknown.retain(|u| !picked.contains(&u.rel_path))
                        });
                    }
                    let inbox = first_inbox(env.root);
                    let send = ui
                        .add_enabled(
                            n > 0 && inbox.is_some(),
                            egui::Button::new(format!("Send {n} to Intake")),
                        )
                        .on_disabled_hover_text(
                            "Intake needs an inbox folder (Intake › Settings).",
                        );
                    if send.clicked()
                        && let Some(inbox) = inbox
                    {
                        let r = sweep::refile_unknown(
                            env.conn,
                            env.root,
                            &inbox,
                            env.device_id,
                            &chosen,
                        )
                        .map(|v| v.len());
                        let picked = s.picked.clone();
                        done(s, r, "sent to Intake's inbox", |d| {
                            d.unknown.retain(|u| !picked.contains(&u.rel_path))
                        });
                    }
                });
            }
            if !d.unreadable.is_empty() {
                section(ui, &format!("Could not be read ({})", d.unreadable.len()));
                for u in &d.unreadable {
                    ui.label(RichText::new(u).small().color(ERROR));
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
