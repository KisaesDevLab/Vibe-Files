//! A client's notes in Browse's details pane (owner 2026-11-15, Q-115, PA-76): the text under
//! `[Memo]` of FileCenter's `!fcinfo.ini` in the client's folder, shown when the client itself
//! is selected, edited here and saved back into that file (`vf_fs::fcinfo`). The file is on the
//! share: it is read and written on a thread, with a spinner.

use std::collections::BTreeMap;

use egui::RichText;
use vf_fs::fcinfo::{self, Notes, Saved};

use crate::ui::browse::{Browse, Env};
use crate::ui::job::{self, Job};
use crate::ui::theme;
use crate::ui::widgets::UiExt as _;

#[derive(Default)]
pub struct NotesState {
    /// The client folder (cabinet-relative) these are the notes of.
    folder: Option<String>,
    /// The notes as read, `None` until they are.
    pub shown: Option<Notes>,
    /// The text in the box.
    pub text: String,
    /// The file changed since it was shown and a save stopped: the file's notes now.
    pub newer: Option<Notes>,
    load: Option<Job<(String, Result<Notes, String>)>>,
    save: Option<Job<(String, Result<Saved, String>)>>,
    pub error: Option<String>,
    /// Text typed for a client and not saved, kept while another client is looked at.
    drafts: BTreeMap<String, String>,
    /// The frame the notes were last drawn in: coming back to them reads the file again.
    drawn: u64,
}

impl NotesState {
    /// Whether the box has text that is not in the file.
    pub fn unsaved(&self) -> bool {
        self.shown.as_ref().is_some_and(|s| s.memo != self.text)
    }

    fn start_load(&mut self, b_ctx: &egui::Context, root: &std::path::Path, folder: &str) {
        let (dir, f) = (root.join(folder), folder.to_string());
        match Job::start(b_ctx, "Reading the notes…", move || {
            let r = fcinfo::read(&dir);
            (f, r)
        }) {
            Ok(j) => self.load = Some(j),
            Err(e) => self.error = Some(format!("The notes were not read: {e}")),
        }
    }

    fn start_save(&mut self, ctx: &egui::Context, root: &std::path::Path, force: bool) {
        let (Some(folder), Some(shown)) = (self.folder.clone(), self.shown.clone()) else {
            return;
        };
        let (root, dir, text) = (root.to_path_buf(), root.join(&folder), self.text.clone());
        match Job::start(ctx, "Saving the notes…", move || {
            let r = fcinfo::save(&root, &dir, &shown, &text, force);
            (folder, r)
        }) {
            Ok(j) => self.save = Some(j),
            Err(e) => self.error = Some(format!("The notes were not saved: {e}")),
        }
    }

    /// The notes of another folder are wanted: what was typed for this one is kept.
    fn turn_to(&mut self, folder: &str) {
        if let Some(old) = self.folder.take()
            && self.unsaved()
        {
            self.drafts.insert(old, std::mem::take(&mut self.text));
        }
        self.folder = Some(folder.to_string());
        self.shown = None;
        self.newer = None;
        self.error = None;
        self.text.clear();
    }
}

/// The notes area for client `ci`, drawn when the client itself (not a folder of it) is what is
/// selected.
pub fn show(ui: &mut egui::Ui, b: &mut Browse, env: &mut Env, ci: usize) {
    let c = &b.snap.clients[ci];
    let Some(folder) = c.folder_rel.clone().filter(|f| !f.is_empty()) else {
        return;
    };
    let client_id = c.id.clone();
    let ctx = ui.ctx().clone();
    let n = &mut b.notes;
    let frame = ctx.cumulative_frame_nr();
    let back = frame > n.drawn + 1;
    n.drawn = frame;
    if n.folder.as_deref() != Some(folder.as_str()) {
        n.turn_to(&folder);
        n.load = None;
        n.start_load(&ctx, env.root, &folder);
    } else if back && n.load.is_none() && n.save.is_none() && !n.unsaved() {
        // Looked away (another folder, a document, another tab) and back: FileCenter or
        // another PC may have changed the notes meanwhile.
        n.start_load(&ctx, env.root, &folder);
    }
    if let Some(r) = job::take(&mut n.load) {
        match r {
            Ok((f, Ok(notes))) if n.folder.as_deref() == Some(f.as_str()) => {
                n.text = n.drafts.remove(&f).unwrap_or_else(|| notes.memo.clone());
                n.shown = Some(notes);
                n.newer = None;
            }
            Ok((f, Err(e))) if n.folder.as_deref() == Some(f.as_str()) => n.error = Some(e),
            Ok(_) => {}
            Err(e) => n.error = Some(format!("The notes were not read: {e}")),
        }
    }
    if let Some(r) = job::take(&mut n.save) {
        match r {
            Ok((f, Ok(saved))) if n.folder.as_deref() == Some(f.as_str()) => match saved {
                Saved::Done(notes) => {
                    n.text = notes.memo.clone();
                    n.shown = Some(notes);
                    n.newer = None;
                    n.error = None;
                    audit(env, &client_id);
                    b.notice = Some("Notes saved.".into());
                }
                Saved::Changed(now) => n.newer = Some(now),
            },
            Ok((f, Err(e))) if n.folder.as_deref() == Some(f.as_str()) => {
                n.error = Some(format!("The notes were not saved: {e}"));
            }
            Ok(_) => {}
            Err(e) => n.error = Some(format!("The notes were not saved: {e}")),
        }
    }
    let n = &mut b.notes;

    ui.add_space(12.0);
    ui.separator();
    let heading = ui.label(RichText::new("NOTES").small().color(theme::ink_muted()));
    if let Some(j) = &n.load {
        j.show(ui);
    }
    if let Some(j) = &n.save {
        j.show(ui);
    }
    if let Some(e) = &n.error {
        ui.colored_label(theme::error(), e);
    }
    let Some(shown) = n.shown.clone() else {
        if n.error.is_some() && n.load.is_none() && ui.small_btn("Try again").clicked() {
            n.error = None;
            n.start_load(&ctx, env.root, &folder);
        }
        return;
    };
    // As tall as the notes: egui grows the box with its lines; three when there are few.
    let rows = n.text.lines().count().max(3);
    // Not typed into while the file is being read again (the read would replace the text).
    ui.add_enabled(
        n.load.is_none(),
        egui::TextEdit::multiline(&mut n.text)
            .desired_width(f32::INFINITY)
            .desired_rows(rows)
            .hint_text("Notes for this client"),
    )
    .labelled_by(heading.id);
    let busy = n.save.is_some() || n.load.is_some();
    let changed = n.unsaved();
    let (mut save, mut revert, mut reload) = (false, false, false);
    ui.horizontal_wrapped(|ui| {
        let label = if shown.exists {
            "Save notes"
        } else {
            "Create notes"
        };
        save = ui
            .add_enabled(changed && !busy, egui::Button::new(label))
            .on_hover_text(format!(
                "Kept in {} in the client's folder, where FileCenter reads them too",
                fcinfo::NAME
            ))
            .clicked();
        revert = ui
            .add_enabled(changed && !busy, egui::Button::new("Revert"))
            .on_hover_text("Back to the notes as they are in the file")
            .clicked();
        reload = ui
            .add_enabled(!busy, egui::Button::new("Reload"))
            .on_hover_text("Read the notes from the file again")
            .clicked();
        if changed {
            ui.label(RichText::new("Not saved").color(theme::warn()));
        }
    });
    let mut force = false;
    if let Some(newer) = n.newer.clone() {
        ui.add_space(4.0);
        ui.colored_label(
            theme::warn(),
            "These notes were changed somewhere else (FileCenter or another PC) since they were shown. The file now has:",
        );
        ui.label(if newer.memo.is_empty() {
            "(no notes)"
        } else {
            newer.memo.as_str()
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Take theirs")
                .on_hover_text("Shows the file's notes; what you typed is dropped")
                .clicked()
            {
                n.text = newer.memo.clone();
                n.shown = Some(newer.clone());
                n.newer = None;
            }
            force = ui
                .add_enabled(!busy, egui::Button::new("Save mine anyway"))
                .on_hover_text("Writes what you typed over the notes in the file")
                .clicked();
        });
    }
    if revert {
        n.text = shown.memo.clone();
        n.newer = None;
    }
    if reload {
        if changed {
            n.drafts.insert(folder.clone(), n.text.clone());
        }
        n.error = None;
        n.start_load(&ctx, env.root, &folder);
    }
    if save || force {
        n.error = None;
        n.start_save(&ctx, env.root, force);
    }
}

/// The audit trail says the notes changed (never what they say).
fn audit(env: &mut Env, client_id: &str) {
    use vf_core::tx::{Actor, NoFileOps, Transaction};
    let r = Transaction::new("client_notes")
        .audit(vf_core::audit::AuditEntry::new(
            "client_notes",
            None,
            serde_json::json!({ "client": client_id }),
        ))
        .commit(env.conn, &mut NoFileOps, &Actor::device(env.device_id));
    if let Err(e) = r {
        tracing::warn!(
            kind = crate::logging::kind(&e.to_string()),
            "notes audit row"
        );
    }
}
