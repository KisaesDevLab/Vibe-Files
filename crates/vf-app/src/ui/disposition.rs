//! Admin › Disposition and Admin › Legal holds (PHASES P07 steps 2–4).
//!
//! Disposition: the "eligible for disposition" report by client and class, approve (destroyed
//! after 30 days) or defer (12 months) per line, what waits for destruction, "Destroy due now"
//! with the firm PIN, and the runs with their certificates. Review it monthly; nothing runs on
//! its own.

use std::collections::BTreeSet;
use std::path::PathBuf;

use egui::{RichText, TextEdit};
use vf_fs::disposition::{self, Line, Report};

use crate::ui::browse::Env;
use crate::ui::{theme, widgets, wizard::ERROR};

#[derive(Default)]
pub struct DispositionPage {
    report: Option<Report>,
    picked: BTreeSet<String>,
    pin: String,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The catalog changed: Browse re-reads.
    pub changed: bool,
    /// A certificate to open (the app runs it through Browse's shell).
    pub open_request: Option<PathBuf>,
}

impl DispositionPage {
    pub fn refresh(&mut self) {
        self.report = None;
    }
}

pub fn show(ui: &mut egui::Ui, p: &mut DispositionPage, env: &mut Env) {
    let today = vf_core::retention::today();
    if p.report.is_none() {
        match disposition::eligible(env.conn, env.root, today) {
            Ok(r) => {
                let live: BTreeSet<String> = r.lines.iter().map(|l| l.doc_id.clone()).collect();
                p.picked.retain(|id| live.contains(id));
                p.report = Some(r);
            }
            Err(e) => {
                p.error = Some(e);
                p.report = Some(Report::default());
            }
        }
    }
    let rep = p.report.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Disposition").strong().size(16.0));
        ui.label(widgets::caption(
            "Documents whose retention has ended. Review monthly: approve (destroyed after 30 days) or defer (12 months).",
        ));
    });
    ui.label(widgets::caption(format!(
        "{} eligible · {} under a legal hold · {} approved or deferred · {} with no retention class (never disposed)",
        rep.lines.len(),
        rep.held,
        rep.decided,
        rep.unclassified
    )));
    if let Some(n) = &p.notice {
        ui.label(RichText::new(n).color(theme::INK_SECONDARY));
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(ERROR));
    }
    ui.separator();
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.label(RichText::new("Eligible for disposition").strong());
        if rep.lines.is_empty() {
            ui.label(RichText::new("Nothing has reached the end of its retention.").color(theme::INK_MUTED));
        } else {
            ui.horizontal(|ui| {
                if ui.small_button("Select all").clicked() {
                    p.picked = rep.lines.iter().map(|l| l.doc_id.clone()).collect();
                }
                if ui.small_button("Select none").clicked() {
                    p.picked.clear();
                }
            });
            let mut client: Option<&str> = None;
            for l in &rep.lines {
                let c = l.client.as_deref().unwrap_or("No client");
                if client != Some(c) {
                    ui.add_space(4.0);
                    ui.label(RichText::new(c).strong());
                    client = Some(c);
                }
                ui.horizontal(|ui| {
                    let mut on = p.picked.contains(&l.doc_id);
                    let name = RichText::new(&l.rel_path).monospace().small();
                    if ui.checkbox(&mut on, name).changed() {
                        if on {
                            p.picked.insert(l.doc_id.clone());
                        } else {
                            p.picked.remove(&l.doc_id);
                        }
                    }
                    ui.label(widgets::caption(format!(
                        "{} · {} · {} · eligible since {}",
                        l.doc_type,
                        l.class,
                        l.tax_year.map_or("no year".to_string(), |y| y.to_string()),
                        l.eligible_on
                    )));
                });
            }
            let chosen: Vec<Line> = rep
                .lines
                .iter()
                .filter(|l| p.picked.contains(&l.doc_id))
                .cloned()
                .collect();
            let n = chosen.len();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(n > 0, egui::Button::new(format!("Approve {n}")))
                    .on_hover_text(format!(
                        "Destroyed once {} days have passed (a hold placed meanwhile still stops it).",
                        disposition::WAIT_DAYS
                    ))
                    .clicked()
                {
                    match disposition::approve(env.conn, env.device_id, &chosen, today) {
                        Ok(n) => {
                            p.notice = Some(format!(
                                "Approved {n}; they are destroyed after {} days.",
                                disposition::WAIT_DAYS
                            ));
                            p.error = None;
                            p.refresh();
                        }
                        Err(e) => p.error = Some(e),
                    }
                }
                if ui
                    .add_enabled(n > 0, egui::Button::new(format!("Defer {n}")))
                    .on_hover_text(format!(
                        "Off the report for {} months.",
                        disposition::DEFER_MONTHS
                    ))
                    .clicked()
                {
                    match disposition::defer(env.conn, env.device_id, &chosen, today) {
                        Ok(n) => {
                            p.notice = Some(format!("Deferred {n}."));
                            p.error = None;
                            p.refresh();
                        }
                        Err(e) => p.error = Some(e),
                    }
                }
            });
        }

        ui.add_space(10.0);
        ui.label(RichText::new("Waiting for destruction").strong());
        let pending = disposition::pending(env.conn).unwrap_or_default();
        let today_s = today.to_string();
        let due = pending.iter().filter(|x| x.destroy_after <= today_s).count();
        if pending.is_empty() {
            ui.label(RichText::new("Nothing approved.").color(theme::INK_MUTED));
        }
        for x in &pending {
            ui.label(
                RichText::new(format!(
                    "{}  ·  destroyed from {}  ·  approved by {}",
                    x.rel_path,
                    x.destroy_after,
                    x.decided_by.as_deref().unwrap_or("—")
                ))
                .small(),
            );
        }
        if due > 0 {
            ui.horizontal(|ui| {
                if vf_fs::trash::has_pin(env.root) {
                    let l = ui.label("Firm PIN");
                    ui.add(TextEdit::singleline(&mut p.pin).password(true).desired_width(80.0))
                        .labelled_by(l.id);
                    let go = ui
                        .add_enabled(
                            !p.pin.is_empty(),
                            egui::Button::new(format!("Destroy {due} due now")),
                        )
                        .on_hover_text("Overwrites and deletes every copy; files a certificate. Cannot be undone.");
                    if go.clicked() {
                        if !vf_fs::trash::check_pin(env.root, &p.pin) {
                            p.error = Some(match vf_fs::trash::pin_wait() {
                                0 => "That PIN is not right.".to_string(),
                                w => format!("Too many wrong PINs: try again in {w} s."),
                            });
                        } else {
                            let name = vf_fs::cabinet::read(env.root)
                                .map(|c| c.display_name(env.root))
                                .unwrap_or_default();
                            match disposition::destroy_due(
                                env.conn,
                                env.root,
                                env.device_id,
                                today,
                                &name,
                            ) {
                                Ok(r) => {
                                    tracing::info!(
                                        documents = r.destroyed,
                                        copies = r.copies,
                                        held = r.held,
                                        failed = r.failed.len(),
                                        "disposition run"
                                    );
                                    let mut msg = format!(
                                        "Destroyed {} documents ({} other copies); the certificate is filed.",
                                        r.destroyed, r.copies
                                    );
                                    if r.held > 0 {
                                        msg.push_str(&format!(" {} kept: under a legal hold.", r.held));
                                    }
                                    if !r.failed.is_empty() {
                                        msg.push_str(&format!(
                                            " Not destroyed (try again): {}",
                                            r.failed.join("; ")
                                        ));
                                    }
                                    p.notice = Some(msg);
                                    p.error = None;
                                    p.changed = true;
                                    p.refresh();
                                }
                                Err(e) => p.error = Some(e),
                            }
                        }
                        p.pin.clear();
                    }
                } else {
                    ui.label(widgets::caption(
                        "Set the firm PIN (Admin › Trash) to destroy approved documents.",
                    ));
                }
            });
        }

        ui.add_space(10.0);
        ui.label(RichText::new("Runs").strong());
        let runs = disposition::runs(env.conn, None).unwrap_or_default();
        if runs.is_empty() {
            ui.label(RichText::new("None yet.").color(theme::INK_MUTED));
        }
        for r in runs {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} · {} documents · {}",
                    widgets::local_time(&r.ts),
                    r.documents,
                    r.by_user.as_deref().unwrap_or("—")
                ));
                if ui.small_button("Open certificate").clicked() {
                    p.open_request = Some(env.root.join(&r.certificate));
                }
            });
        }
    });
}

/// Admin › Legal holds: every hold in the cabinet, active first, with Release.
#[derive(Default)]
pub struct HoldsPage {
    pub notice: Option<String>,
    pub error: Option<String>,
}

pub fn holds(ui: &mut egui::Ui, p: &mut HoldsPage, env: &mut Env) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Legal holds").strong().size(16.0));
        ui.label(widgets::caption(
            "Kept in _Vibe\\legal_holds.json, so every workstation honors them. Place one from Browse (a client, or selected documents).",
        ));
    });
    if let Some(n) = &p.notice {
        ui.label(RichText::new(n).color(theme::INK_SECONDARY));
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(ERROR));
    }
    ui.separator();
    let mut all = match vf_fs::holds::load(env.root) {
        Ok(h) => h,
        Err(e) => {
            ui.label(RichText::new(e).color(ERROR));
            return;
        }
    };
    all.sort_by_key(|h| (!h.active(), std::cmp::Reverse(h.since.clone())));
    if all.is_empty() {
        ui.label(RichText::new("No legal holds.").color(theme::INK_MUTED));
        return;
    }
    let names: std::collections::HashMap<String, String> = env
        .conn
        .prepare("SELECT id, name || ' (' || ext_id || ')' FROM client")
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()
        })
        .unwrap_or_default();
    let mut release = None;
    for h in &all {
        ui.horizontal_wrapped(|ui| {
            let what = match &h.client_id {
                Some(c) => format!(
                    "Client {}",
                    names.get(c).cloned().unwrap_or_else(|| c.clone())
                ),
                None => format!("{} documents", h.docs.len()),
            };
            let line = format!(
                "{what} · {} · since {} by {}",
                h.reason,
                widgets::local_time(&h.since),
                h.placed_by.as_deref().unwrap_or("—")
            );
            if h.active() {
                ui.label(RichText::new(line).color(ERROR));
                if ui.small_button("Release").clicked() {
                    release = Some(h.id.clone());
                }
            } else {
                ui.label(
                    RichText::new(format!(
                        "{line} · released {}",
                        h.released
                            .as_deref()
                            .map(widgets::local_time)
                            .unwrap_or_default()
                    ))
                    .color(theme::INK_MUTED),
                );
            }
        });
    }
    if let Some(id) = release {
        match vf_fs::holds::release(env.conn, env.root, env.device_id, &id) {
            Ok(()) => p.notice = Some("Legal hold released.".into()),
            Err(e) => p.error = Some(e),
        }
    }
}

/// Admin › Safeguards appendix (P07 step 5).
#[derive(Default)]
pub struct SafeguardsPage {
    /// Start of the period (yyyy-mm-dd); empty = a year ago.
    pub from: String,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The PDF just written, to open.
    pub open_request: Option<PathBuf>,
}

pub fn safeguards(ui: &mut egui::Ui, p: &mut SafeguardsPage, env: &mut Env) {
    if p.from.is_empty() {
        p.from = vf_core::rfc3339_days_ago(365)[..10].to_string();
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Safeguards appendix").strong().size(16.0));
        ui.label(widgets::caption(
            "One PDF for the WISP: inventory, encryption, workstations, retention and holds, integrity, backup, disposition runs.",
        ));
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let l = ui.label("Period from");
        ui.add(
            TextEdit::singleline(&mut p.from)
                .hint_text("yyyy-mm-dd")
                .desired_width(100.0),
        )
        .labelled_by(l.id);
        if ui
            .add(widgets::primary_button("Export appendix…"))
            .clicked()
            && let Some(dest) = rfd::FileDialog::new()
                .set_file_name("Safeguards appendix.pdf")
                .add_filter("PDF", &["pdf"])
                .save_file()
        {
            let name = vf_fs::cabinet::read(env.root)
                .map(|c| c.display_name(env.root))
                .unwrap_or_default();
            let from = format!("{}T00:00:00Z", p.from.trim());
            let r = vf_fs::safeguards::appendix(
                env.conn,
                env.root,
                &vf_fs::safeguards::Inputs {
                    cabinet_name: &name,
                    period_from: &from,
                },
            )
            .and_then(|bytes| std::fs::write(&dest, bytes).map_err(|e| e.to_string()));
            match r {
                Ok(()) => {
                    p.notice = Some(format!("Saved {}", dest.display()));
                    p.error = None;
                    p.open_request = Some(dest);
                }
                Err(e) => p.error = Some(e),
            }
        }
    });
    if let Some(n) = &p.notice {
        ui.label(RichText::new(n).color(theme::INK_SECONDARY));
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(ERROR));
    }
}
