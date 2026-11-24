//! Tax return delivery (ADDENDUM-tax-return-delivery.md §12, owner 2026-11-24; Vibe-Portal
//! D1/D2): the client panel's Deliver tax return… window and Admin › Portal › Deliveries.
//! Slice 2 builds the return step (a file already on the portal) and the Form 8879 step (the
//! client's UltraTax eSignature link, Confirm signed); letters, package and fee come in later
//! slices. Each delivery read is copied into the client's folder with an audit row when it
//! changed; every portal call runs on a thread with "Working…" shown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_portal::PortalSettings;
use vf_portal::api::PortalFile;
use vf_portal::delivery::{self, Delivery, DeliveryChange, NewDelivery, StepsIn};

use crate::portal as core;
use crate::ui::portal::{AuditOut, ClientRef, spawn};
use crate::ui::{theme, widgets};

// ---- The copy in the client's folder -----------------------------------------------------------

/// Deliveries read from the portal, for the app to copy into their client folders.
#[derive(Clone)]
pub struct ReadOut(pub(crate) Sender<Vec<Delivery>>);

impl ReadOut {
    pub(crate) fn send(&self, d: Vec<Delivery>) {
        if !d.is_empty() {
            let _ = self.0.send(d);
        }
    }
}

/// Writes the copies of deliveries read (only into a folder still bound to the client, only
/// when changed), in one Transaction with an audit row each (`portal_delivery_copy`).
pub fn record_reads(
    conn: &mut vf_core::db::Connection,
    root: &Path,
    device_id: &str,
    reads: Vec<Vec<Delivery>>,
) -> Result<(), String> {
    let (pc, user) = core::who();
    let by = format!("{pc}\\{user}");
    let now = vf_core::now_rfc3339();
    let mut tx = vf_core::tx::Transaction::new("portal_delivery_copy").record_only();
    let mut any = false;
    let mut folders: HashMap<String, Option<String>> = HashMap::new();
    for d in reads.into_iter().flatten() {
        let folder = folders
            .entry(d.client_id.clone())
            .or_insert_with(|| {
                conn.query_row(
                    "SELECT folder_rel FROM client WHERE id = ?1 AND deleted_at IS NULL",
                    [&d.client_id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten()
                .filter(|f| {
                    filer_intake::sentinel::read_sentinel(&root.join(f))
                        .ok()
                        .flatten()
                        .map(|s| s.client_id)
                        == uuid::Uuid::parse_str(&d.client_id).ok()
                })
            })
            .clone();
        let Some(folder) = folder else { continue };
        let path = delivery::copy_path(root, &folder, &d.id);
        let last = delivery::newest_copy(&path);
        let Some(line) = delivery::copy_line(last.as_ref(), &d, &now, &by) else {
            continue;
        };
        any = true;
        tx = tx
            .file(vf_core::tx::FileOp::AppendLines {
                path,
                lines: vec![line],
                undo_lines: Vec::new(),
            })
            .audit(AuditEntry::new(
                "portal_delivery_copy",
                None,
                serde_json::json!({
                    "client_id": d.client_id,
                    "delivery_id": d.id,
                    "status": d.status,
                }),
            ));
    }
    if !any {
        return Ok(());
    }
    let mut fs = vf_fs::fileops::FilerFileOps::new(&[root]).map_err(|e| e.to_string())?;
    tx.commit(conn, &mut fs, &vf_core::tx::Actor::device(device_id))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ---- The Deliver tax return window -------------------------------------------------------------

enum Done {
    Loaded(Result<(Vec<Delivery>, Vec<PortalFile>), vf_portal::PortalError>),
    Saved(Result<Box<Delivery>, String>, &'static str),
    Said(Result<String, String>),
}

enum Act {
    Send,
    Confirm,
    Remind,
    Cancel,
    Report,
    Reload,
}

pub struct DeliverWindow {
    pub closed: bool,
    client: ClientRef,
    root: PathBuf,
    folder: Option<String>,
    deliveries: Vec<Delivery>,
    files: Vec<PortalFile>,
    offline: Option<String>,
    // The form.
    year: i32,
    return_type: String,
    title: String,
    with_return: bool,
    with_8879: bool,
    return_file: Option<String>,
    link: String,
    notify: bool,
    confirm_cancel: bool,
    job: Option<Receiver<Done>>,
    said: Option<(String, bool)>,
    err: Option<String>,
    audit: AuditOut,
    reads: ReadOut,
}

/// Last calendar year: the tax year a return delivered now usually is.
fn default_year() -> i32 {
    use chrono::Datelike;
    chrono::Local::now().year() - 1
}

impl DeliverWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: &egui::Context,
        s: &PortalSettings,
        root: PathBuf,
        client: ClientRef,
        folder: Option<String>,
        audit: AuditOut,
        reads: ReadOut,
    ) -> Self {
        let year = default_year();
        let mut w = Self {
            closed: false,
            client,
            root,
            folder,
            deliveries: Vec::new(),
            files: Vec::new(),
            offline: None,
            year,
            return_type: "1040".into(),
            title: format!("Your {year} tax return"),
            with_return: true,
            with_8879: true,
            return_file: None,
            link: String::new(),
            notify: true,
            confirm_cancel: false,
            job: None,
            said: None,
            err: None,
            audit,
            reads,
        };
        w.load(ctx, s);
        w
    }

    fn load(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        let s2 = s.clone();
        let id = self.client.id.clone();
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s2);
            Done::Loaded((|| {
                let mut ds = Vec::new();
                for d in p.deliveries(Some(&id))? {
                    // With its events (the list read leaves them out).
                    ds.push(p.delivery(&d.id)?);
                }
                let files = p.files_for_client(&id)?;
                Ok((ds, files))
            })())
        }));
    }

    /// The delivery this window is about: the open one for the year and return type chosen.
    fn current(&self) -> Option<&Delivery> {
        self.deliveries.iter().find(|d| {
            d.status == "open" && d.tax_year == self.year && d.return_type == self.return_type
        })
    }

    /// Fills the form from the delivery shown (or a default return file for a new one).
    fn fill(&mut self) {
        if let Some(d) = self.current().cloned() {
            self.title = d.title.clone();
            self.with_return = d.step(delivery::RETURN).is_some_and(|s| s.present);
            self.with_8879 = d.step(delivery::ESIGN).is_some_and(|s| s.present);
            self.return_file = d.return_file.as_ref().map(|f| f.id.clone());
            self.link = d.esign_link.clone().unwrap_or_default();
            self.notify = d.notify;
        } else if self.return_file.is_none() {
            // The newest file on the portal for the year whose name says it is the return.
            let ty = self.return_type.clone();
            self.return_file = self
                .files
                .iter()
                .filter(|f| f.tax_year == Some(self.year))
                .find(|f| {
                    let n = f.name.to_lowercase();
                    n.contains(&ty.to_lowercase()) || n.contains("return")
                })
                .map(|f| f.file_id.clone());
        }
    }

    fn take_done(&mut self) {
        let Some(rx) = &self.job else { return };
        let Ok(done) = rx.try_recv() else { return };
        self.job = None;
        match done {
            Done::Loaded(Ok((ds, files))) => {
                self.reads.send(ds.clone());
                self.deliveries = ds;
                let mut files = files;
                files.sort_by(|a, b| b.shared_at.cmp(&a.shared_at));
                self.files = files;
                self.offline = None;
                self.fill();
            }
            Done::Loaded(Err(e)) => {
                if matches!(e, vf_portal::PortalError::Network(_))
                    && let Some(f) = &self.folder
                {
                    let copies = delivery::read_copies(&self.root, f);
                    if let Some(newest) = copies.iter().map(|c| c.read_at.clone()).max() {
                        self.offline = Some(format!(
                            "The portal cannot be reached: as last read {}.",
                            widgets::local_time(&newest)
                        ));
                        self.deliveries = copies.into_iter().map(|c| c.delivery).collect();
                        self.fill();
                    }
                }
                self.err = Some(e.message());
            }
            Done::Saved(Ok(d), what) => {
                let d = *d;
                self.audit.send(AuditEntry::new(
                    what,
                    None,
                    serde_json::json!({
                        "client_id": d.client_id,
                        "delivery_id": d.id,
                        "fields": ["steps", "return_file", "esign_link", "title", "notify"],
                    }),
                ));
                self.said = Some((
                    match what {
                        "portal_delivery_create" => "Sent to the client.".into(),
                        "portal_delivery_confirm_8879" => "Form 8879 confirmed.".into(),
                        "portal_delivery_cancel" => {
                            "Cancelled: the card has left the portal.".into()
                        }
                        _ => "Saved.".into(),
                    },
                    false,
                ));
                self.err = None;
                self.reads.send(vec![d.clone()]);
                match self.deliveries.iter().position(|x| x.id == d.id) {
                    Some(i) => self.deliveries[i] = d,
                    None => self.deliveries.insert(0, d),
                }
            }
            Done::Said(Ok(m)) => {
                self.said = Some((m, false));
                self.err = None;
            }
            Done::Saved(Err(e), _) | Done::Said(Err(e)) => self.err = Some(e),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        self.take_done();
        let busy = self.job.is_some();
        let mut open_win = true;
        let mut act = None;
        egui::Window::new(format!(
            "Deliver tax return — {} ({})",
            self.client.name, self.client.code
        ))
        .id(egui::Id::unique("portal-delivery"))
        .open(&mut open_win)
        .collapsible(false)
        .default_width(900.0)
        .default_height(820.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| act = self.body(ui, busy));
        });
        if !open_win {
            self.closed = true;
        }
        if let Some(a) = act {
            self.apply(ctx, s, a);
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, busy: bool) -> Option<Act> {
        let mut act = None;
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Working…");
            });
        }
        if let Some(o) = &self.offline {
            ui.colored_label(theme::warn(), o);
        }
        let current = self.current().cloned();
        let sent = current.is_some();
        let can = !busy && self.offline.is_none();
        // The strip: year, return type, title, status.
        ui.horizontal_wrapped(|ui| {
            ui.label("Tax year");
            let before = (self.year, self.return_type.clone());
            ui.add_enabled(can, egui::DragValue::new(&mut self.year).range(1990..=2100));
            ui.label("Return type");
            ui.add_enabled_ui(can, |ui| {
                egui::ComboBox::from_id_salt("delivery-type")
                    .selected_text(&self.return_type)
                    .show_ui(ui, |ui| {
                        for t in delivery::RETURN_TYPES {
                            ui.selectable_value(&mut self.return_type, t.to_string(), t);
                        }
                    });
            });
            if before != (self.year, self.return_type.clone()) {
                if self.current().is_none() {
                    self.title = format!("Your {} tax return", self.year);
                    self.return_file = None;
                }
                self.fill();
            }
            let (word, colour) = match current.as_ref().map(|d| d.status.as_str()) {
                Some("open") => ("Sent", theme::accent()),
                Some("complete") => ("Complete", theme::health_ok()),
                Some("cancelled") => ("Cancelled", theme::ink_muted()),
                _ => ("Not sent yet", theme::ink_muted()),
            };
            ui.label(RichText::new(word).strong().color(colour));
        });
        ui.horizontal(|ui| {
            ui.label("Card title");
            ui.add_enabled(
                can,
                egui::TextEdit::singleline(&mut self.title)
                    .desired_width(320.0)
                    .char_limit(120),
            );
        });
        ui.add_space(6.0);
        let step_state = |kind: &str| -> String {
            current
                .as_ref()
                .and_then(|d| d.step(kind))
                .filter(|s| s.present)
                .map(|s| delivery::state_words(&s.state).to_string())
                .unwrap_or_default()
        };
        // Section: the tax return PDF.
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.add_enabled(
                    can && !current
                        .as_ref()
                        .and_then(|d| d.step(delivery::RETURN))
                        .is_some_and(|s| s.state == "done"),
                    egui::Checkbox::new(&mut self.with_return, RichText::new("Tax return PDF").strong()),
                );
                ui.label(RichText::new(step_state(delivery::RETURN)).color(theme::ink_muted()));
            });
            if self.with_return {
                let shown = self
                    .return_file
                    .as_deref()
                    .and_then(|id| self.files.iter().find(|f| f.file_id == id))
                    .map(|f| f.name.clone())
                    .unwrap_or_else(|| "Choose the return…".into());
                ui.add_enabled_ui(can, |ui| {
                    egui::ComboBox::from_id_salt("delivery-return")
                        .selected_text(shown)
                        .width(420.0)
                        .show_ui(ui, |ui| {
                            for f in &self.files {
                                ui.selectable_value(
                                    &mut self.return_file,
                                    Some(f.file_id.clone()),
                                    format!(
                                        "{}{}",
                                        f.name,
                                        f.tax_year.map(|y| format!(" ({y})")).unwrap_or_default()
                                    ),
                                );
                            }
                        });
                });
                ui.label(widgets::caption(
                    "Files on the portal for this client. Share the return first (right-click the \
                     document › Share on portal…), then choose it here. The client can open it when \
                     the card's earlier steps allow.",
                ));
            }
        });
        // Section: Form 8879.
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.add_enabled(
                    can && !current
                        .as_ref()
                        .and_then(|d| d.step(delivery::ESIGN))
                        .is_some_and(|s| s.state == "done"),
                    egui::Checkbox::new(&mut self.with_8879, RichText::new("Form 8879").strong()),
                );
                ui.label(RichText::new(step_state(delivery::ESIGN)).color(theme::ink_muted()));
            });
            if self.with_8879 {
                ui.horizontal(|ui| {
                    ui.label("UltraTax eSignature link");
                    widgets::named(
                        ui.add_enabled(
                            can,
                            egui::TextEdit::singleline(&mut self.link)
                                .desired_width(380.0)
                                .hint_text("https://…"),
                        ),
                        "eSignature link",
                    );
                });
                let waiting = current
                    .as_ref()
                    .and_then(|d| d.step(delivery::ESIGN))
                    .is_some_and(|s| s.state == "waiting");
                if waiting {
                    ui.colored_label(theme::warn(), "The client says they signed it.");
                }
                if ui
                    .add_enabled(can && waiting, egui::Button::new("Confirm signed"))
                    .on_hover_text("You checked UltraTax: Form 8879 is signed")
                    .clicked()
                {
                    act = Some(Act::Confirm);
                }
            }
        });
        ui.checkbox(
            &mut self.notify,
            "Tell the client by email and text when it is sent",
        );
        let link_ok = !self.with_8879
            || self.link.trim().is_empty()
            || self.link.trim().to_lowercase().starts_with("https://");
        if !link_ok {
            ui.colored_label(theme::error(), "The eSignature link starts with https://.");
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    can && sent,
                    egui::Button::new(RichText::new("Cancel delivery…").color(theme::error())),
                )
                .clicked()
            {
                self.confirm_cancel = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if sent {
                    "Save changes"
                } else {
                    "Send to client"
                };
                if ui
                    .add_enabled(
                        can && link_ok && (self.with_return || self.with_8879),
                        widgets::primary_button(label),
                    )
                    .clicked()
                {
                    act = Some(Act::Send);
                }
                if ui
                    .add_enabled(can && sent, egui::Button::new("Remind now"))
                    .clicked()
                {
                    act = Some(Act::Remind);
                }
                if ui
                    .add_enabled(!busy && sent, egui::Button::new("Download report…"))
                    .clicked()
                {
                    act = Some(Act::Report);
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("Read again"))
                    .clicked()
                {
                    act = Some(Act::Reload);
                }
            });
        });
        if self.confirm_cancel {
            ui.group(|ui| {
                ui.label("Cancel this delivery? The card leaves the client's portal. What was done stays in its log.");
                ui.horizontal(|ui| {
                    if ui.button("Cancel delivery").clicked() {
                        self.confirm_cancel = false;
                        act = Some(Act::Cancel);
                    }
                    if ui.button("Keep it").clicked() {
                        self.confirm_cancel = false;
                    }
                });
            });
        }
        if let Some((m, warn)) = &self.said {
            ui.colored_label(
                if *warn {
                    theme::warn()
                } else {
                    theme::health_ok()
                },
                m,
            );
        }
        if let Some(e) = &self.err {
            ui.colored_label(theme::error(), e);
        }
        ui.separator();
        ui.label(
            RichText::new("WHAT HAS HAPPENED")
                .small()
                .color(theme::ink_muted()),
        );
        match &current {
            Some(d) if !d.events.is_empty() => {
                for e in &d.events {
                    ui.label(
                        RichText::new(format!(
                            "{}  {}{}",
                            widgets::local_time(&e.at),
                            e.words,
                            if e.by.is_empty() {
                                String::new()
                            } else {
                                format!(" · {}", e.by)
                            }
                        ))
                        .small(),
                    );
                }
            }
            _ => {
                ui.label(widgets::caption("Nothing yet."));
            }
        }
        ui.label(widgets::caption(format!(
            "Each finished step's PDF, with its log page, is filed in the client's Income Tax\\{}\\Delivery folder.",
            self.year
        )));
        act
    }

    fn apply(&mut self, ctx: &egui::Context, s: &PortalSettings, act: Act) {
        self.said = None;
        self.err = None;
        let s2 = s.clone();
        let current = self.current().cloned();
        match act {
            Act::Reload => self.load(ctx, s),
            Act::Send => {
                let steps = StepsIn {
                    return_: self.with_return,
                    esign8879: self.with_8879,
                    ..Default::default()
                };
                let title = self.title.trim().to_string();
                let file = self.return_file.clone().filter(|_| self.with_return);
                let link = self.link.trim().to_string();
                let link = (self.with_8879 && !link.is_empty()).then_some(link);
                let notify = self.notify;
                let new = NewDelivery {
                    client_id: self.client.id.clone(),
                    tax_year: self.year,
                    return_type: self.return_type.clone(),
                    title: title.clone(),
                    steps: steps.clone(),
                    locks: delivery::Locks {
                        return_after_letters: true,
                        esign_after_letters: true,
                        esign_after_fee: true,
                    },
                    assigned_staff_id: None,
                    notify,
                    remind_days: 0,
                };
                let what = if current.is_none() {
                    "portal_delivery_create"
                } else {
                    "portal_delivery_change"
                };
                self.job = Some(spawn(ctx, move || {
                    let p = core::client(&s2);
                    let r = (|| {
                        let mut d = match &current {
                            None => p.create_delivery(&new)?,
                            Some(d) => p.change_delivery(
                                &d.id,
                                &DeliveryChange {
                                    title: Some(title),
                                    steps: Some(steps),
                                    notify: Some(notify),
                                    ..Default::default()
                                },
                            )?,
                        };
                        if let Some(f) = file
                            && d.return_file.as_ref().map(|r| r.id.as_str()) != Some(f.as_str())
                        {
                            d = p.set_delivery_return(&d.id, &f)?;
                        }
                        if let Some(l) = link
                            && d.esign_link.as_deref() != Some(l.as_str())
                        {
                            d = p.set_esign_link(&d.id, &l)?;
                        }
                        p.delivery(&d.id)
                    })();
                    Done::Saved(r.map(Box::new).map_err(|e| e.message()), what)
                }));
            }
            Act::Confirm => {
                let Some(d) = current else { return };
                self.job = Some(spawn(ctx, move || {
                    let p = core::client(&s2);
                    let r = p.confirm_8879(&d.id).and_then(|x| p.delivery(&x.id));
                    Done::Saved(
                        r.map(Box::new).map_err(|e| e.message()),
                        "portal_delivery_confirm_8879",
                    )
                }));
            }
            Act::Cancel => {
                let Some(d) = current else { return };
                self.job = Some(spawn(ctx, move || {
                    let p = core::client(&s2);
                    let r = p.cancel_delivery(&d.id).and_then(|x| p.delivery(&x.id));
                    Done::Saved(
                        r.map(Box::new).map_err(|e| e.message()),
                        "portal_delivery_cancel",
                    )
                }));
            }
            Act::Remind => {
                let Some(d) = current else { return };
                let audit = self.audit.clone();
                self.job = Some(spawn(ctx, move || {
                    Done::Said(
                        core::client(&s2)
                            .remind_delivery(&d.id)
                            .map(|n| {
                                audit.send(AuditEntry::new(
                                    "portal_delivery_remind",
                                    None,
                                    serde_json::json!({ "client_id": d.client_id, "delivery_id": d.id, "told": n }),
                                ));
                                match n {
                                    0 => "Nobody could be told: invite someone to the portal first.".to_string(),
                                    1 => "1 message sent.".into(),
                                    n => format!("{n} messages sent."),
                                }
                            })
                            .map_err(|e| e.message()),
                    )
                }));
            }
            Act::Report => {
                let Some(d) = current else { return };
                let Some(dest) = rfd::FileDialog::new()
                    .set_title("Save the delivery report")
                    .set_file_name(format!(
                        "{} {} Delivery report - {}.pdf",
                        d.tax_year, d.return_type, self.client.code
                    ))
                    .add_filter("PDF", &["pdf"])
                    .save_file()
                else {
                    return;
                };
                if dest.starts_with(&self.root) {
                    self.err = Some("Save the report outside the cabinet; the step PDFs are filed in the client's folder already.".into());
                    return;
                }
                let audit = self.audit.clone();
                self.job = Some(spawn(ctx, move || {
                    Done::Said(
                        core::client(&s2)
                            .delivery_report(&d.id)
                            .map_err(|e| e.message())
                            .and_then(|bytes| {
                                filer_core::datafile::write_atomic(&dest, &bytes)
                                    .map_err(|e| format!("The report could not be saved: {e}"))
                            })
                            .map(|()| {
                                audit.send(AuditEntry::new(
                                    "portal_delivery_report",
                                    None,
                                    serde_json::json!({ "client_id": d.client_id, "delivery_id": d.id }),
                                ));
                                format!("Saved {}.", dest.display())
                            }),
                    )
                }));
            }
        }
    }
}

// ---- Admin › Portal › Deliveries ---------------------------------------------------------------

#[derive(Default)]
pub struct DeliveriesPage {
    filter: Option<&'static str>,
    data: Option<Vec<Delivery>>,
    clients: HashMap<String, (ClientRef, Option<String>)>,
    job: Option<Receiver<Result<Vec<Delivery>, String>>>,
    loaded: bool,
    err: Option<String>,
    window: Option<DeliverWindow>,
    audit: Option<AuditOut>,
    reads: Option<ReadOut>,
}

impl DeliveriesPage {
    pub fn reread(&mut self) {
        self.loaded = false;
    }
    pub fn connected(&self) -> bool {
        self.audit.is_some()
    }
    pub fn connect(&mut self, audit: AuditOut, reads: ReadOut) {
        self.audit = Some(audit);
        self.reads = Some(reads);
    }
}

fn client_rows(env: &mut crate::ui::browse::Env) -> HashMap<String, (ClientRef, Option<String>)> {
    let Ok(mut stmt) = env
        .conn
        .prepare("SELECT id, ext_id, name, folder_rel FROM client WHERE deleted_at IS NULL")
    else {
        return HashMap::new();
    };
    stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            (
                ClientRef {
                    id: r.get(0)?,
                    code: r.get(1)?,
                    name: r.get(2)?,
                },
                r.get::<_, Option<String>>(3)?,
            ),
        ))
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Whole days since an RFC 3339 time.
fn days_since(at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_days())
        .unwrap_or(0)
}

pub fn page(ui: &mut egui::Ui, page: &mut DeliveriesPage, env: &mut crate::ui::browse::Env) {
    ui.heading("Deliveries");
    let settings = vf_fs::cabinet::read(env.root)
        .ok()
        .and_then(|c| vf_portal::settings::load(env.root, &c.cabinet_id));
    let Some(s) = settings.filter(|s| s.deliveries) else {
        ui.label("Tax return delivery is off for this cabinet. Turn it on in Admin › Portal.");
        return;
    };
    ui.label(widgets::caption(
        "Every open tax return delivery and the step it waits on: the season's worklist. Click a \
         client to open its Deliver tax return window.",
    ));
    let ctx = ui.ctx().clone();
    if let Some(rx) = &page.job
        && let Ok(r) = rx.try_recv()
    {
        page.job = None;
        match r {
            Ok(d) => {
                if let Some(o) = &page.reads {
                    o.send(d.clone());
                }
                page.data = Some(d);
                page.err = None;
            }
            Err(e) => page.err = Some(e),
        }
    }
    if !page.loaded {
        page.loaded = true;
        page.clients = client_rows(env);
        let s2 = s.clone();
        page.job = Some(spawn(&ctx, move || {
            core::client(&s2).deliveries(None).map_err(|e| e.message())
        }));
    }
    ui.horizontal(|ui| {
        ui.selectable_value(&mut page.filter, None, "All open");
        for (k, name) in [
            (delivery::RETURN, "Tax return"),
            (delivery::ESIGN, "Form 8879"),
        ] {
            ui.selectable_value(&mut page.filter, Some(k), name);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(page.job.is_none(), egui::Button::new("Read again"))
                .clicked()
            {
                page.loaded = false;
            }
            if page.job.is_some() {
                ui.spinner();
            }
        });
    });
    if let Some(e) = &page.err {
        ui.colored_label(theme::error(), e);
    }
    let mut open: Option<String> = None;
    if let Some(list) = &page.data {
        let rows: Vec<&Delivery> = list
            .iter()
            .filter(|d| d.status == "open")
            .filter(|d| {
                page.filter
                    .is_none_or(|f| d.waiting().is_some_and(|s| s.kind == f))
            })
            .collect();
        if rows.is_empty() {
            ui.label(widgets::caption("None."));
        }
        egui::Grid::new("deliveries-list")
            .num_columns(5)
            .striped(true)
            .spacing([14.0, 6.0])
            .show(ui, |ui| {
                for h in ["Client", "Year", "Type", "Step waiting", "Days"] {
                    ui.label(RichText::new(h).small().color(theme::ink_muted()));
                }
                ui.end_row();
                for d in rows {
                    let name = page
                        .clients
                        .get(&d.client_id)
                        .map(|(c, _)| format!("{} ({})", c.name, c.code))
                        .unwrap_or_else(|| "(not in this cabinet)".into());
                    if ui.link(&name).clicked() {
                        open = Some(d.client_id.clone());
                    }
                    ui.label(d.tax_year.to_string());
                    ui.label(&d.return_type);
                    ui.label(
                        d.waiting()
                            .map(|s| format!("{} · {}", s.name, delivery::state_words(&s.state)))
                            .unwrap_or_default(),
                    );
                    ui.label(days_since(&d.created_at).to_string());
                    ui.end_row();
                }
            });
    }
    if let Some(id) = open
        && let Some((c, folder)) = page.clients.get(&id).cloned()
        && let (Some(audit), Some(reads)) = (page.audit.clone(), page.reads.clone())
    {
        page.window = Some(DeliverWindow::new(
            &ctx,
            &s,
            env.root.to_path_buf(),
            c,
            folder,
            audit,
            reads,
        ));
    }
    if let Some(w) = &mut page.window {
        w.show(&ctx, &s);
        if w.closed {
            page.window = None;
            page.loaded = false;
        }
    }
}

/// The client panel's line for one client, from the deliveries last read on this PC.
pub fn panel_line(read: Option<&Vec<Delivery>>, client_id: &str) -> Option<String> {
    let d = read?
        .iter()
        .filter(|d| d.client_id == client_id && d.status == "open")
        .max_by(|a, b| a.created_at.cmp(&b.created_at))?;
    Some(format!("Tax return delivery: {}", d.line()))
}
