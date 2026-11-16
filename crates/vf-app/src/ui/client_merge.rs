//! Merge the client shown in Browse into another (owner 2026-11-14, Q-107): a client made
//! twice under similar names becomes one folder (`vf_fs::clientmerge`). The preview, the
//! merge and its undo each run on their own thread: all three read the cabinet's folders.

use std::path::{Path, PathBuf};

use egui::{RichText, TextEdit};
use vf_core::db::Connection;
use vf_fs::clientmerge::{self, Merged, Plan};

use crate::ui::browse::Env;
use crate::ui::job::{self, Job};
use crate::ui::theme;
use crate::ui::widgets::{self, DialogOutcome};

/// A client to pick: (uuid, "Name  ID").
pub type Pick = (String, String);

/// What the dialog did: the message to show, and the client to show after it.
pub struct Done {
    pub message: String,
    pub show: Option<String>,
}

enum Work {
    Plan(Result<Plan, String>),
    Merged(Result<Merged, String>),
    Undone(Result<(Merged, usize), String>),
}

pub struct MergeDialog {
    /// The client merged away: (uuid, "Name  ID").
    from: Pick,
    /// The other clients with a folder.
    others: Vec<Pick>,
    query: String,
    into: Option<Pick>,
    plan: Option<Result<Plan, String>>,
    /// The cabinet's last merge, when it can still be undone.
    last: Option<Merged>,
    error: Option<String>,
    focused: bool,
    job: Option<Job<Work>>,
}

impl MergeDialog {
    pub fn new(root: &Path, from: Pick, others: Vec<Pick>) -> Self {
        Self {
            from,
            others,
            query: String::new(),
            into: None,
            plan: None,
            last: clientmerge::load_last(root),
            error: None,
            focused: false,
            job: None,
        }
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn show(ctx: &egui::Context, d: &mut MergeDialog, env: &mut Env) -> DialogOutcome<Done> {
    let mut out = DialogOutcome::Open;
    if let Some(r) = job::take(&mut d.job) {
        match r {
            Err(e) => d.error = Some(e),
            Ok(Work::Plan(p)) => d.plan = Some(p),
            Ok(Work::Merged(Err(e))) | Ok(Work::Undone(Err(e))) => {
                d.error = Some(e);
                d.plan = None;
            }
            Ok(Work::Merged(Ok(m))) => {
                let mut message = format!(
                    "Merged {} into {}: {} moved.",
                    m.from.name,
                    m.into.name,
                    plural(m.moves.len(), "document", "documents")
                );
                if let Some(n) = &m.note {
                    message.push(' ');
                    message.push_str(n);
                }
                out = DialogOutcome::Done(Done {
                    message,
                    show: Some(m.into.id.clone()),
                });
            }
            Ok(Work::Undone(Ok((m, back)))) => {
                let left = m.moves.len() - back;
                let mut message = format!(
                    "{} is a client of its own again: {} went back.",
                    m.from.name,
                    plural(back, "document", "documents")
                );
                if left > 0 {
                    message.push_str(&format!(
                        " {} moved or renamed since the merge stayed with {}.",
                        plural(left, "document", "documents"),
                        m.into.name
                    ));
                }
                out = DialogOutcome::Done(Done {
                    message,
                    show: Some(m.from.id.clone()),
                });
            }
        }
    }
    let busy = d.job.is_some();
    egui::Modal::new(egui::Id::unique("client-merge")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading("Merge into another client");
        ui.add_space(6.0);
        ui.label(format!(
            "{} is merged into the client you pick: its documents move to the same folders there, and it is then removed.",
            d.from.1
        ));
        ui.add_space(8.0);
        let l = ui.label("Merge into");
        let r = ui
            .add_enabled(
                !busy,
                TextEdit::singleline(&mut d.query)
                    .hint_text("Type a client's name or ID")
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(l.id);
        if !busy {
            widgets::clear_x(ui, &r, &mut d.query);
        }
        if !d.focused {
            r.request_focus();
            d.focused = true;
        }
        let q = d.query.trim().to_lowercase();
        let mut picked = None;
        if !q.is_empty() && d.into.as_ref().is_none_or(|p| p.1 != d.query) {
            let hits: Vec<&Pick> = d
                .others
                .iter()
                .filter(|c| c.1.to_lowercase().contains(&q))
                .take(8)
                .collect();
            if hits.is_empty() {
                ui.label(widgets::caption("No client with a folder matches."));
            }
            for c in hits {
                if ui.selectable_label(false, &c.1).clicked() {
                    picked = Some(c.clone());
                }
            }
        }
        if r.changed() && d.into.as_ref().is_some_and(|p| p.1 != d.query) {
            d.into = None;
            d.plan = None;
            d.error = None;
        }
        if let Some(p) = picked {
            d.query = p.1.clone();
            d.plan = None;
            d.error = None;
            let (from, into) = (d.from.0.clone(), p.0.clone());
            d.into = Some(p);
            start(ctx, d, env, "Looking at both folders…", move |conn, root, _| {
                Work::Plan(clientmerge::plan(conn, root, &from, &into))
            });
        }
        ui.add_space(8.0);
        match &d.plan {
            Some(Ok(p)) => {
                let to = p.into.folder.as_deref().unwrap_or_default();
                ui.label(format!(
                    "{} to {to}.",
                    match p.moves.len() {
                        0 => "No documents move".to_string(),
                        1 => "1 document moves".to_string(),
                        n => format!("{n} documents move"),
                    }
                ));
                let both = p.kept_both();
                if both > 0 {
                    ui.label(format!(
                        "{} there: both are kept, and the one coming in gets \"(2)\" in its name.",
                        if both == 1 {
                            "1 has a name already used".to_string()
                        } else {
                            format!("{both} have a name already used")
                        }
                    ));
                }
                if p.holds > 0 || p.relationships > 0 {
                    ui.label(format!(
                        "{} and {} become {}'s.",
                        plural(p.holds, "legal hold", "legal holds"),
                        plural(p.relationships, "relationship", "relationships"),
                        p.into.name
                    ));
                }
                ui.label(widgets::caption(format!(
                    "{}'s folder, its place on the client list and its row in the client CSV are removed. Files later named with the ID {} are not matched to a client. To put it back, open this window again: Undo last merge.",
                    p.from.name, p.from.ext_id
                )));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(theme::ink_muted()));
            }
            None => {}
        }
        ui.add_space(6.0);
        widgets::message_line(ui, d.error.as_deref());
        if let Some(j) = &d.job {
            j.show(ui);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let ready = matches!(d.plan, Some(Ok(_)));
            if ui
                .add_enabled(ready && !busy, widgets::primary_button("Merge"))
                .clicked()
                && let Some(Ok(p)) = d.plan.clone()
            {
                d.error = None;
                start(ctx, d, env, "Merging…", move |conn, root, device| {
                    Work::Merged(clientmerge::apply(conn, root, &p, device))
                });
            }
            // A merge cannot be stopped halfway: Cancel waits for it.
            if !busy
                && (ui.button("Cancel").clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape)))
            {
                out = DialogOutcome::Cancelled;
            }
        });
        if let Some(m) = d.last.clone() {
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(widgets::caption(format!(
                    "Last merge: {} into {} ({}).",
                    m.from.name,
                    m.into.name,
                    m.when.get(..10).unwrap_or(&m.when)
                )));
                if ui
                    .add_enabled(!busy, egui::Button::new("Undo last merge"))
                    .on_hover_text(
                        "Its documents go back to a folder of its own, with its place on the client list",
                    )
                    .clicked()
                {
                    d.error = None;
                    start(ctx, d, env, "Putting the client back…", move |conn, root, device| {
                        Work::Undone(clientmerge::undo(conn, root, &m, device).map(|n| (m, n)))
                    });
                }
            });
        }
    });
    out
}

/// Runs `work` on its own thread with its own catalog connection (on this one, at once, for
/// a catalog with no file).
fn start(
    ctx: &egui::Context,
    d: &mut MergeDialog,
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
