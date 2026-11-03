//! The Sync tab (PHASES P11 steps 8–9): set up (or join) the cabinet's B2 sync, unlock it with
//! the sync passphrase once per session (held in memory only, never logged or saved), then:
//! status, pending per client, last push / pull, errors, pause / resume, Push now, Pull now,
//! Verify now (sampled download + SHA-256), Conflicts (keep mine / theirs / both), restore the
//! cabinet or one client to a folder, and the Object Lock setting. Work runs on a thread with
//! its own catalog connection; pushes repeat on their own while unlocked and not paused.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::RichText;
use vf_core::db::Storage;
use vf_sync::Sync;
use vf_sync::restore::{Conflict, Keep};

use crate::config::SyncConfig;
use crate::ui::browse::Env;
use crate::ui::{theme, widgets, wizard::ERROR};

/// Documents downloaded and checked by Verify now.
pub const VERIFY_SAMPLE: usize = 20;

pub enum Done {
    Setup(Sync, String),
    Push(vf_sync::push::PushReport),
    Pull(vf_sync::pull::PullReport),
    Verify(usize, usize),
    Restored(vf_sync::restore::RestoreReport, PathBuf),
    Exported(usize, PathBuf),
}

/// What runs on the sync thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Push(Option<String>),
    Pull,
    Verify,
    Restore(PathBuf),
    Export(String, PathBuf),
}

#[derive(Default)]
struct Form {
    endpoint: String,
    region: String,
    bucket: String,
    key_id: String,
    app_key: String,
    pass: String,
    pass2: String,
    unlock: String,
}

#[derive(Default)]
pub struct SyncTab {
    pub session: Option<Sync>,
    job: Option<(String, Receiver<Result<Done, String>>)>,
    /// The client a running push or pull is for (`Some("")` = all).
    pub running_for: Option<String>,
    /// Pending documents per client id (`""` = the cabinet), refreshed every few seconds.
    pub pending: BTreeMap<String, usize>,
    pub conflicts: Vec<Conflict>,
    pub last_push: Option<String>,
    pub last_pull: Option<String>,
    pub head_rev: u64,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The catalog changed (a pull): Browse re-reads.
    pub changed: bool,
    /// New settings for `config.toml` (the app saves them).
    pub config_changed: Option<SyncConfig>,
    /// Show only this client in the pending table (from Browse's pill).
    pub filter_client: Option<String>,
    form: Form,
    refreshed: Option<Instant>,
    last_auto: Option<Instant>,
}

fn computer_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "this PC".into())
}

impl SyncTab {
    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    /// Starts `op` on the sync thread.
    pub fn start(&mut self, ctx: &egui::Context, catalog: &Path, root: &Path, op: Op) {
        let Some(s) = self.session.clone() else {
            self.error = Some("Unlock sync first (Sync tab).".into());
            return;
        };
        if self.running() {
            return;
        }
        self.error = None;
        self.running_for = match &op {
            Op::Push(c) => Some(c.clone().unwrap_or_default()),
            Op::Pull => Some(String::new()),
            _ => None,
        };
        let label = match &op {
            Op::Push(Some(_)) => "Pushing this client…",
            Op::Push(None) => "Pushing…",
            Op::Pull => "Pulling…",
            Op::Verify => "Verifying a sample…",
            Op::Restore(_) => "Restoring the cabinet…",
            Op::Export(..) => "Restoring the client…",
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let (catalog, root, ctx) = (catalog.to_path_buf(), root.to_path_buf(), ctx.clone());
        std::thread::Builder::new()
            .name("b2-sync".into())
            .spawn(move || {
                let r = run(&catalog, &root, &s, op);
                let _ = tx.send(r);
                ctx.request_repaint();
            })
            .ok();
        self.job = Some((label.into(), rx));
    }

    /// Every frame (whatever the tab): picks up finished work, refreshes the counts, and starts
    /// the automatic push when it is due.
    pub fn tick(&mut self, ctx: &egui::Context, env: &Env, catalog: &Path, cfg: &SyncConfig) {
        if let Some((_, rx)) = &self.job {
            match rx.try_recv() {
                Err(TryRecvError::Empty) => {}
                r => {
                    self.job = None;
                    self.running_for = None;
                    self.refreshed = None;
                    match r.unwrap_or_else(|_| Err("the sync work stopped".into())) {
                        Ok(d) => self.finished(d, cfg),
                        Err(e) => {
                            tracing::warn!(kind = crate::logging::kind(&e), "sync failed");
                            self.error = Some(e);
                        }
                    }
                }
            }
        }
        if self
            .refreshed
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(3))
        {
            self.refresh(env);
        }
        if self.session.is_some() && self.last_auto.is_none() {
            // The first automatic push comes one interval after unlocking.
            self.last_auto = Some(Instant::now());
        }
        if self.session.is_some()
            && !cfg.paused
            && cfg.auto_minutes > 0
            && !self.running()
            && self
                .last_auto
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(60 * cfg.auto_minutes as u64))
        {
            self.last_auto = Some(Instant::now());
            let hour = chrono::Timelike::hour(&chrono::Local::now()) as u8;
            if cfg.in_window(hour) {
                self.start(ctx, catalog, env.root, Op::Push(None));
            }
        }
    }

    fn refresh(&mut self, env: &Env) {
        self.refreshed = Some(Instant::now());
        self.pending = vf_sync::catalog::pending(env.conn).unwrap_or_default();
        self.conflicts = vf_sync::restore::conflicts(env.conn).unwrap_or_default();
        let st: Option<(i64, Option<String>, Option<String>)> = env
            .conn
            .query_row(
                "SELECT last_manifest_rev, last_push, last_pull FROM sync_state WHERE device_id = ?1",
                [env.device_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        if let Some((rev, p, l)) = st {
            self.head_rev = rev as u64;
            self.last_push = p;
            self.last_pull = l;
        }
    }

    fn finished(&mut self, d: Done, cfg: &SyncConfig) {
        match d {
            Done::Setup(s, sealed) => {
                self.session = Some(s);
                let mut c = cfg.clone();
                c.app_key_sealed = sealed;
                self.config_changed = Some(c);
                self.notice =
                    Some("Sync is set up. The first push sends the whole cabinet.".into());
            }
            Done::Push(r) => {
                if r.pulled.as_ref().is_some_and(|p| p.changed() > 0) {
                    self.changed = true;
                }
                let conflicts = r.pulled.as_ref().map_or(0, |p| p.conflicts);
                self.notice = Some(format!(
                    "Pushed {} documents ({} new objects, {}) · manifest rev {}{}",
                    r.documents,
                    r.objects,
                    widgets::size(r.bytes as i64),
                    r.head_rev,
                    if conflicts > 0 {
                        format!(" · {conflicts} conflicts to resolve")
                    } else {
                        String::new()
                    }
                ));
            }
            Done::Pull(r) => {
                if r.changed() > 0 {
                    self.changed = true;
                }
                self.notice = Some(format!(
                    "Pulled: {} new, {} updated, {} moved, {} re-typed, {} deleted, {} conflicts{}",
                    r.new,
                    r.updated,
                    r.moved,
                    r.meta,
                    r.deleted,
                    r.conflicts,
                    if r.deferred > 0 {
                        format!(" · {} wait for Intake's last batch", r.deferred)
                    } else {
                        String::new()
                    }
                ));
            }
            Done::Verify(n, bad) => {
                if bad == 0 {
                    self.notice = Some(format!(
                        "Verified: {n} documents downloaded and checked, all match."
                    ));
                } else {
                    self.error = Some(format!(
                        "Verify: {bad} of {n} documents in the bucket do not match."
                    ));
                }
            }
            Done::Restored(r, p) => {
                self.notice = Some(format!(
                    "Restored {} clients, {} documents to {}. Open it as a cabinet to use it.",
                    r.clients,
                    r.documents,
                    p.display()
                ));
            }
            Done::Exported(n, p) => {
                self.notice = Some(format!("Restored {n} documents to {}.", p.display()));
            }
        }
    }

    /// Opens sync with the passphrase (no network: the key and the sealed application key are
    /// local).
    fn unlock(&mut self, root: &Path, cfg: &SyncConfig, device_id: &str) {
        let pass = std::mem::take(&mut self.form.unlock);
        let r = (|| -> Result<Sync, String> {
            let keys = vf_sync::setup::unlock_local(root, &cfg.cabinet_id, &pass)
                .map_err(|e| e.to_string())?;
            let app_key = vf_sync::setup::open_secret(&keys, &cfg.app_key_sealed)
                .map_err(|e| e.to_string())?;
            let store = vf_sync::Remote::s3(&cfg.s3(), &app_key).map_err(|e| e.to_string())?;
            Ok(Sync {
                remote: vf_sync::Remote::new(store, &cfg.cabinet_id),
                keys,
                cabinet_id: cfg.cabinet_id.clone(),
                device_id: device_id.into(),
                device_name: computer_name(),
            })
        })();
        match r {
            Ok(s) => {
                self.session = Some(s);
                self.error = None;
                self.notice = Some("Sync unlocked for this session.".into());
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn setup(&mut self, ctx: &egui::Context, root: &Path, device_id: &str, cabinet_id: &str) {
        let f = &mut self.form;
        if f.pass != f.pass2 {
            self.error = Some("The two passphrases differ.".into());
            return;
        }
        let cfg = SyncConfig {
            endpoint: f.endpoint.trim().into(),
            region: f.region.trim().into(),
            bucket: f.bucket.trim().into(),
            key_id: f.key_id.trim().into(),
            cabinet_id: cabinet_id.into(),
            ..SyncConfig::default()
        };
        let (app_key, pass) = (std::mem::take(&mut f.app_key), std::mem::take(&mut f.pass));
        f.pass2.clear();
        let (tx, rx) = std::sync::mpsc::channel();
        let (root, device, ctx) = (root.to_path_buf(), device_id.to_string(), ctx.clone());
        let cab = cabinet_id.to_string();
        self.config_changed = Some(cfg.clone());
        std::thread::Builder::new()
            .name("b2-setup".into())
            .spawn(move || {
                let r = (|| -> Result<Done, String> {
                    let store =
                        vf_sync::Remote::s3(&cfg.s3(), &app_key).map_err(|e| e.to_string())?;
                    let remote = vf_sync::Remote::new(store, &cab);
                    let keys =
                        vf_sync::setup::init(&root, &remote, &pass, vf_sync::setup::DEFAULT_KDF)
                            .map_err(|e| e.to_string())?;
                    let sealed = vf_sync::setup::seal_secret(&keys, &app_key);
                    Ok(Done::Setup(
                        Sync {
                            remote,
                            keys,
                            cabinet_id: cab,
                            device_id: device,
                            device_name: computer_name(),
                        },
                        sealed,
                    ))
                })();
                let _ = tx.send(r);
                ctx.request_repaint();
            })
            .ok();
        self.job = Some(("Connecting to the bucket…".into(), rx));
    }
}

fn run(catalog: &Path, root: &Path, s: &Sync, op: Op) -> Result<Done, String> {
    let e = |e: vf_sync::SyncError| e.to_string();
    match op {
        Op::Verify => vf_sync::restore::verify(s, VERIFY_SAMPLE)
            .map(|(n, b)| Done::Verify(n, b))
            .map_err(e),
        Op::Export(client, dest) => vf_sync::restore::export_client(s, &client, &dest)
            .map(|n| Done::Exported(n, dest))
            .map_err(e),
        Op::Restore(dest) => {
            let tmp = tempfile_catalog(catalog)?;
            let mut conn = vf_core::db::open(&tmp, Storage::Local).map_err(|e| e.to_string())?;
            let r = vf_sync::restore::restore_session(&mut conn, &dest, s);
            drop(conn);
            let _ = std::fs::remove_file(&tmp);
            r.map(|r| Done::Restored(r, dest)).map_err(e)
        }
        Op::Push(only) => {
            let mut conn = vf_core::db::open(catalog, Storage::Local).map_err(|e| e.to_string())?;
            vf_sync::push::push(&mut conn, root, s, only.as_deref())
                .map(Done::Push)
                .map_err(e)
        }
        Op::Pull => {
            let mut conn = vf_core::db::open(catalog, Storage::Local).map_err(|e| e.to_string())?;
            vf_sync::pull::pull(&mut conn, root, s)
                .map(Done::Pull)
                .map_err(e)
        }
    }
}

fn tempfile_catalog(catalog: &Path) -> Result<PathBuf, String> {
    let dir = catalog.parent().ok_or("no data folder")?.join("tmp");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("restore-{}.db", vf_core::new_id())))
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, password: bool, hint: &str) {
    ui.horizontal(|ui| {
        let l = ui.add_sized([150.0, 20.0], egui::Label::new(label));
        ui.add(
            egui::TextEdit::singleline(value)
                .password(password)
                .hint_text(hint)
                .desired_width(360.0),
        )
        .labelled_by(l.id);
    });
}

pub struct View<'a> {
    pub catalog: &'a Path,
    pub cabinet_id: &'a str,
    pub cfg: &'a SyncConfig,
    /// (id, name) of every client, for the pending table.
    pub clients: &'a [(String, String)],
}

pub fn show(ui: &mut egui::Ui, t: &mut SyncTab, env: &mut Env, v: View<'_>) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.heading("Sync");
        ui.label(widgets::caption(
            "Two-way sync of this cabinet with your Backblaze B2 bucket. Everything is encrypted on \
             this PC before it leaves; the bucket's names reveal no clients. The sync passphrase is \
             the only way to the key: keep it with the firm's recovery papers.",
        ));
        ui.add_space(6.0);
        if let Some((what, _)) = &t.job {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(what.as_str());
            });
        }
        if let Some(n) = &t.notice {
            ui.label(RichText::new(n).color(theme::INK));
        }
        if let Some(e) = &t.error {
            ui.label(RichText::new(e).color(ERROR));
        }
        ui.add_space(6.0);
        let configured = v.cfg.configured_for(v.cabinet_id);
        if !configured {
            setup_form(ui, t, env, &v);
        } else if t.session.is_none() {
            ui.label("Sync is set up for this cabinet. Unlock it for this session:");
            field(ui, "Sync passphrase", &mut t.form.unlock, true, "");
            if ui.add(widgets::primary_button("Unlock")).clicked() {
                t.unlock(env.root, v.cfg, env.device_id);
            }
        } else {
            unlocked(ui, t, env, &v);
        }
    });
}

fn setup_form(ui: &mut egui::Ui, t: &mut SyncTab, env: &Env, v: &View<'_>) {
    ui.label(RichText::new("Set up sync").strong());
    ui.label(widgets::caption(
        "From B2: the bucket's S3 endpoint and region, and an application key for it. On a second \
         workstation, enter the same bucket and the firm's existing sync passphrase to join.",
    ));
    let f = &mut t.form;
    field(
        ui,
        "S3 endpoint",
        &mut f.endpoint,
        false,
        "https://s3.us-west-004.backblazeb2.com",
    );
    field(ui, "Region", &mut f.region, false, "us-west-004");
    field(ui, "Bucket", &mut f.bucket, false, "");
    field(ui, "Key ID", &mut f.key_id, false, "");
    field(ui, "Application key", &mut f.app_key, true, "");
    field(
        ui,
        "Sync passphrase",
        &mut f.pass,
        true,
        "at least 12 characters",
    );
    field(ui, "Passphrase again", &mut f.pass2, true, "");
    let ready = !f.endpoint.trim().is_empty()
        && !f.bucket.trim().is_empty()
        && !f.key_id.trim().is_empty()
        && !f.app_key.is_empty()
        && !f.pass.is_empty();
    if ui
        .add_enabled(ready && !t.running(), widgets::primary_button("Connect"))
        .clicked()
    {
        t.setup(ui.ctx(), env.root, env.device_id, v.cabinet_id);
    }
}

fn when(ts: &Option<String>) -> String {
    ts.as_deref()
        .map(widgets::local_time)
        .unwrap_or_else(|| "never".into())
}

fn unlocked(ui: &mut egui::Ui, t: &mut SyncTab, env: &mut Env, v: &View<'_>) {
    let total: usize = t.pending.values().sum();
    egui::Grid::new("sync-status")
        .num_columns(2)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            ui.label("Status");
            ui.label(if t.running() {
                "working".to_string()
            } else if v.cfg.paused {
                "paused".to_string()
            } else if !t.conflicts.is_empty() {
                format!("{} conflicts", t.conflicts.len())
            } else if total > 0 {
                format!("{total} pending")
            } else {
                "in sync".to_string()
            });
            ui.end_row();
            ui.label("Bucket");
            ui.label(format!("{} · {}", v.cfg.bucket, v.cfg.endpoint));
            ui.end_row();
            ui.label("Last push");
            ui.label(when(&t.last_push));
            ui.end_row();
            ui.label("Last pull");
            ui.label(when(&t.last_pull));
            ui.end_row();
            ui.label("Manifest rev");
            ui.label(t.head_rev.to_string());
            ui.end_row();
            ui.label("Automatic");
            ui.label(if v.cfg.auto_minutes == 0 {
                "by hand only".to_string()
            } else {
                format!(
                    "every {} minutes{}",
                    v.cfg.auto_minutes,
                    v.cfg
                        .window
                        .map(|(a, b)| format!(", {a:02}:00–{b:02}:00"))
                        .unwrap_or_default()
                )
            });
            ui.end_row();
        });
    ui.add_space(6.0);
    let idle = !t.running();
    ui.horizontal(|ui| {
        if ui.add_enabled(idle, widgets::primary_button("Push now")).clicked() {
            t.start(ui.ctx(), v.catalog, env.root, Op::Push(None));
        }
        if ui.add_enabled(idle, egui::Button::new("Pull now")).clicked() {
            t.start(ui.ctx(), v.catalog, env.root, Op::Pull);
        }
        if ui
            .add_enabled(idle, egui::Button::new("Verify now"))
            .on_hover_text(format!(
                "Downloads {VERIFY_SAMPLE} documents spread over the bucket and checks each against its SHA-256."
            ))
            .clicked()
        {
            t.start(ui.ctx(), v.catalog, env.root, Op::Verify);
        }
        let mut cfg = v.cfg.clone();
        if ui
            .button(if cfg.paused { "Resume" } else { "Pause" })
            .clicked()
        {
            cfg.paused = !cfg.paused;
            t.config_changed = Some(cfg);
        }
        if ui
            .add_enabled(idle, egui::Button::new("Restore cabinet to a folder…"))
            .on_hover_text("Rebuilds the whole cabinet from the bucket into an empty folder.")
            .clicked()
            && let Some(p) = rfd::FileDialog::new().pick_folder()
        {
            t.start(ui.ctx(), v.catalog, env.root, Op::Restore(p));
        }
        if ui.button("Lock").on_hover_text("Forget the key until unlocked again.").clicked() {
            t.session = None;
        }
    });
    ui.add_space(8.0);

    if !t.conflicts.is_empty() {
        ui.label(RichText::new(format!("Conflicts ({})", t.conflicts.len())).strong());
        let mut act: Option<(String, Keep)> = None;
        for c in &t.conflicts {
            ui.horizontal_wrapped(|ui| {
                ui.label(&c.rel_path);
                match &c.copy_rel_path {
                    Some(copy) => {
                        ui.label(widgets::caption(format!(
                            "changed here and on another workstation; this PC's version is {}",
                            widgets::file_name(copy)
                        )));
                        if ui.button("Keep mine").clicked() {
                            act = Some((c.id.clone(), Keep::Mine));
                        }
                        if ui.button("Keep theirs").clicked() {
                            act = Some((c.id.clone(), Keep::Theirs));
                        }
                        if ui.button("Keep both").clicked() {
                            act = Some((c.id.clone(), Keep::Both));
                        }
                    }
                    None => {
                        ui.label(widgets::caption(
                            "deleted on another workstation, changed here: kept",
                        ));
                        if ui.button("OK").clicked() {
                            act = Some((c.id.clone(), Keep::Both));
                        }
                    }
                }
            });
        }
        if let Some((id, keep)) = act {
            match vf_sync::restore::resolve(env.conn, env.root, env.device_id, &id, keep) {
                Ok(()) => {
                    t.changed = true;
                    t.refreshed = None;
                }
                Err(e) => t.error = Some(e.to_string()),
            }
        }
        ui.add_space(8.0);
    }

    ui.horizontal(|ui| {
        ui.label(RichText::new("Pending by client").strong());
        if let Some(c) = &t.filter_client {
            let name = v
                .clients
                .iter()
                .find(|(id, _)| id == c)
                .map(|(_, n)| n.as_str())
                .unwrap_or("?");
            ui.label(widgets::caption(format!("showing {name}")));
            if ui.small_button("Show all").clicked() {
                t.filter_client = None;
            }
        }
    });
    let mut go: Option<Op> = None;
    egui::Grid::new("sync-pending")
        .striped(true)
        .num_columns(3)
        .show(ui, |ui| {
            let names: BTreeMap<&str, &str> = v
                .clients
                .iter()
                .map(|(i, n)| (i.as_str(), n.as_str()))
                .collect();
            for (id, n) in &t.pending {
                if t.filter_client.as_ref().is_some_and(|f| f != id) {
                    continue;
                }
                ui.label(if id.is_empty() {
                    "Cabinet (not in a client folder, master files)"
                } else {
                    names.get(id.as_str()).copied().unwrap_or("(client)")
                });
                ui.label(format!("{n} pending"));
                if !id.is_empty() {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(idle, egui::Button::new("Push")).clicked() {
                            go = Some(Op::Push(Some(id.clone())));
                        }
                        if ui
                            .add_enabled(idle, egui::Button::new("Restore to a folder…"))
                            .clicked()
                            && let Some(p) = rfd::FileDialog::new().pick_folder()
                        {
                            go = Some(Op::Export(id.clone(), p));
                        }
                    });
                } else {
                    ui.label("");
                }
                ui.end_row();
            }
        });
    if t.pending.is_empty() {
        ui.label(widgets::caption(
            "Nothing waiting: this PC and the bucket agree.",
        ));
    }
    if let Some(op) = go {
        t.start(ui.ctx(), v.catalog, env.root, op);
    }

    ui.add_space(10.0);
    ui.label(RichText::new("Settings").strong());
    let mut cfg = v.cfg.clone();
    let mut changed = false;
    ui.horizontal(|ui| {
        let l = ui.label("Push automatically every (minutes, 0 = by hand)");
        changed |= ui
            .add(egui::DragValue::new(&mut cfg.auto_minutes).range(0..=1440))
            .labelled_by(l.id)
            .changed();
    });
    ui.horizontal(|ui| {
        let mut on = cfg.window.is_some();
        if ui
            .checkbox(&mut on, "Only push automatically between")
            .changed()
        {
            cfg.window = on.then_some((19, 7));
            changed = true;
        }
        if let Some((a, b)) = &mut cfg.window {
            let la = ui.label("from");
            changed |= ui
                .add(egui::DragValue::new(a).range(0..=23).suffix(":00"))
                .labelled_by(la.id)
                .changed();
            let lb = ui.label("to");
            changed |= ui
                .add(egui::DragValue::new(b).range(0..=23).suffix(":00"))
                .labelled_by(lb.id)
                .changed();
        }
    });
    ui.horizontal(|ui| {
        let mut on = cfg.object_lock_days.is_some();
        if ui
            .checkbox(&mut on, "B2 Object Lock is on for this bucket")
            .on_hover_text(
                "Object Lock makes every object in the bucket impossible to change or delete for a \
                 number of days, even with the application key: ransomware or a mistake on a \
                 workstation cannot destroy the offsite copy. It is set on the bucket in B2 \
                 (Bucket Settings › Object Lock). Suggested: 30 days. Disposition (P07) cannot \
                 remove a locked object before its days are up.",
            )
            .changed()
        {
            cfg.object_lock_days = on.then_some(30);
            changed = true;
        }
        if let Some(d) = &mut cfg.object_lock_days {
            let l = ui.label("days");
            changed |= ui
                .add(egui::DragValue::new(d).range(1..=3650))
                .labelled_by(l.id)
                .changed();
        }
    });
    if changed {
        if let Some(s) = &t.session {
            // Recorded in the bucket's cabinet.json for other workstations (off the UI thread).
            let (remote, days) = (s.remote.clone(), cfg.object_lock_days);
            std::thread::spawn(move || {
                let _ = vf_sync::setup::set_object_lock(&remote, days);
            });
        }
        t.config_changed = Some(cfg);
    }
}
