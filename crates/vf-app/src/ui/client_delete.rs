//! "Delete client…" (owner 2026-11-24): a client whose folder holds no documents goes, folder
//! and all (`vf_fs::clientdelete`). The check, the delete and its undo each run on their own
//! thread: they read the cabinet's folders.

use std::path::{Path, PathBuf};

use egui::RichText;
use vf_core::db::Connection;
use vf_fs::clientdelete::{self, Deleted, Plan};

use crate::ui::browse::Env;
use crate::ui::job::{self, Job};
use crate::ui::theme;
use crate::ui::widgets::{self, DialogOutcome};

enum Work {
    Plan(Result<Plan, String>),
    Deleted(Box<Result<Deleted, String>>),
}

pub struct DeleteDialog {
    /// The client: (uuid, "Name  ID").
    client: (String, String),
    /// The portal is set up for this cabinet (said: nothing there changes).
    portal: bool,
    plan: Option<Result<Plan, String>>,
    error: Option<String>,
    job: Option<Job<Work>>,
    started: bool,
}

impl DeleteDialog {
    pub fn new(client: (String, String), portal: bool) -> Self {
        Self {
            client,
            portal,
            plan: None,
            error: None,
            job: None,
            started: false,
        }
    }
}

/// What a finished delete says, and its record for Undo.
pub fn said(d: &Deleted) -> String {
    let mut m = format!("Deleted {} ({}) and its empty folder.", d.name, d.ext_id);
    if d.folder.is_none() {
        m = format!("Deleted {} ({}).", d.name, d.ext_id);
    }
    if let Some(n) = &d.note {
        m.push(' ');
        m.push_str(n);
    }
    m
}

pub fn show(ctx: &egui::Context, d: &mut DeleteDialog, env: &mut Env) -> DialogOutcome<Deleted> {
    let mut out = DialogOutcome::Open;
    if !d.started {
        d.started = true;
        let id = d.client.0.clone();
        start(
            ctx,
            d,
            env,
            "Looking at the client's folder…",
            move |conn, root, _| Work::Plan(clientdelete::plan(conn, root, &id)),
        );
    }
    if let Some(r) = job::take(&mut d.job) {
        match r {
            Err(e) => d.error = Some(e),
            Ok(Work::Plan(p)) => d.plan = Some(p),
            Ok(Work::Deleted(r)) => match *r {
                Err(e) => d.error = Some(e),
                Ok(x) => out = DialogOutcome::Done(x),
            },
        }
    }
    let busy = d.job.is_some();
    let mut go = false;
    egui::Modal::new(egui::Id::unique("client-delete")).show(ctx, |ui| {
        ui.set_width(540.0);
        ui.heading("Delete client");
        ui.add_space(6.0);
        match &d.plan {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(widgets::caption("Looking at the client's folder…"));
                });
            }
            Some(Err(e)) => {
                ui.label(RichText::new(format!("{} cannot be deleted.", d.client.1)).strong());
                ui.label(RichText::new(e.as_str()).color(theme::warn()));
            }
            Some(Ok(p)) => {
                ui.label(
                    RichText::new(format!("Delete {} ({})?", p.name, p.ext_id)).strong(),
                );
                ui.add_space(4.0);
                match &p.folder {
                    Some(f) => {
                        let empty = p.dirs.len().saturating_sub(1);
                        ui.label(format!(
                            "• Its folder {f} goes{}.",
                            match empty {
                                0 => String::new(),
                                1 => " with the empty folder in it".into(),
                                n => format!(" with the {n} empty folders in it"),
                            }
                        ));
                        if p.own_records {
                            ui.label(
                                "• Vibe Files' own records of it (_Vibe) are kept aside, so Undo can put them back.",
                            );
                        }
                    }
                    None => {
                        ui.label("• It has no folder.");
                    }
                }
                ui.label("• It leaves the client list and Intake's client CSV; a later import brings it back if your tax software still lists it.");
                if p.relationships > 0 {
                    ui.label(format!(
                        "• Its {} relationship{} with other clients go{}.",
                        p.relationships,
                        if p.relationships == 1 { "" } else { "s" },
                        if p.relationships == 1 { "es" } else { "" }
                    ));
                }
                if d.portal {
                    ui.label(
                        "• Nothing changes on the portal: if the client is there, its people and files stay until you remove them there.",
                    );
                }
                ui.add_space(4.0);
                ui.label(widgets::caption(
                    "Undo is offered in Browse right after, and stays until the next delete.",
                ));
            }
        }
        widgets::message_line(ui, d.error.as_deref());
        if let Some(j) = &d.job {
            j.show(ui);
        }
        ui.horizontal(|ui| {
            let ok = matches!(d.plan, Some(Ok(_))) && !busy;
            if ui
                .add_enabled(ok, widgets::primary_button("Delete client"))
                .clicked()
            {
                go = true;
            }
            if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                out = DialogOutcome::Cancelled;
            }
        });
    });
    if go && let Some(Ok(p)) = d.plan.clone() {
        d.error = None;
        start(
            ctx,
            d,
            env,
            "Deleting the client…",
            move |conn, root, device| {
                Work::Deleted(Box::new(clientdelete::apply(conn, root, &p, device)))
            },
        );
    }
    out
}

/// Runs `work` on its own thread with its own catalog connection (on this one, at once, for
/// a catalog with no file).
fn start(
    ctx: &egui::Context,
    d: &mut DeleteDialog,
    env: &mut Env,
    what: &str,
    work: impl FnOnce(&mut Connection, &Path, &str) -> Work + Send + 'static,
) {
    let catalog = env.conn.path().filter(|p| !p.is_empty()).map(PathBuf::from);
    let (root, device) = (env.root.to_path_buf(), env.device_id.to_string());
    let Some(catalog) = catalog else {
        let done = work(env.conn, &root, &device);
        match Job::start(ctx, what, move || done) {
            Ok(j) => d.job = Some(j),
            Err(e) => d.error = Some(e),
        }
        return;
    };
    match Job::start(ctx, what, move || {
        match vf_core::db::open(&catalog, vf_core::db::Storage::Local) {
            Ok(mut conn) => work(&mut conn, &root, &device),
            Err(e) => Work::Plan(Err(e.to_string())),
        }
    }) {
        Ok(j) => d.job = Some(j),
        Err(e) => d.error = Some(e),
    }
}

/// Undoes the last delete (Browse's notice: Undo), on this thread: a few small files.
pub fn undo(env: &mut Env, d: &Deleted) -> Result<String, String> {
    clientdelete::undo(env.conn, env.root, d, env.device_id)?;
    Ok(format!(
        "{} ({}) is back, with its folder.",
        d.name, d.ext_id
    ))
}
