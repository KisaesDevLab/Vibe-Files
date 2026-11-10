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
use zeroize::Zeroizing;

use egui::RichText;
use vf_core::db::Storage;
use vf_sync::Sync;
use vf_sync::restore::{Conflict, Keep};

use crate::config::SyncConfig;
use crate::ui::browse::Env;
use crate::ui::{theme, widgets};

/// Documents downloaded and checked by Verify now.
pub const VERIFY_SAMPLE: usize = 20;

pub enum Done {
    /// (firm key, session, sealed application key).
    Setup(Zeroizing<[u8; 32]>, Sync, String),
    /// (firm key, session): unlocked with the passphrase.
    Unlocked(Zeroizing<[u8; 32]>, Sync),
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

struct Form {
    endpoint: String,
    region: String,
    bucket: String,
    key_id: String,
    app_key: String,
    pass: String,
    pass2: String,
    unlock: String,
    /// Remember the key on this PC for this Windows login (Q-38).
    remember: bool,
    /// Keep the key and the bucket settings in the cabinet for everyone (Q-43).
    share: bool,
}

impl Default for Form {
    fn default() -> Self {
        Form {
            endpoint: String::new(),
            region: String::new(),
            bucket: String::new(),
            key_id: String::new(),
            app_key: String::new(),
            pass: String::new(),
            pass2: String::new(),
            unlock: String::new(),
            remember: true,
            share: true,
        }
    }
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
    /// The counts being made on their own thread (PA-34: they read every document).
    counting: Option<Receiver<Counted>>,
    last_auto: Option<Instant>,
    /// When the remembered key was last tried.
    recall_at: Option<Instant>,
    /// Locked by the person: no automatic unlock until they unlock again.
    locked_by_user: bool,
    /// This login has a remembered key on this PC (for the Forget button; no file check per
    /// frame).
    remembered: bool,
    /// This PC's data folder, for a set-up running on its thread.
    setup_data: PathBuf,
    /// The cabinet, for a set-up or unlock running on its thread (to share the key).
    setup_root: PathBuf,
    /// The settings a set-up running on its thread is for.
    setup_cfg: Option<SyncConfig>,
    /// The cabinet keeps the key for everyone (`_Vibe/keys/sync-shared.json`).
    pub shared: bool,
}

/// Pending per client, open conflicts, and this PC's last exchange (rev, push, pull).
type Counted = (
    BTreeMap<String, usize>,
    Vec<Conflict>,
    Option<(i64, Option<String>, Option<String>)>,
);

const SHARE_LABEL: &str = "Let everyone who opens this cabinet sync without the passphrase";
const SHARE_HELP: &str = "Keeps the sync key and the bucket settings in the cabinet's _Vibe\\keys folder, which is never uploaded: the Backblaze copy stays encrypted, and anyone who can open the cabinet on the network can sync. Keep the passphrase with the firm's recovery papers.";

const REMEMBER_HELP: &str = "The key is sealed with Windows (DPAPI) for your login on this \
computer and kept in this PC's data folder: sync unlocks itself when you start Vibe Files. Another \
login or another computer cannot open it. The passphrase stays the way back in.";

/// This PC's data folder (the catalog's).
fn data_dir(catalog: &Path) -> &Path {
    catalog.parent().unwrap_or(Path::new("."))
}

fn computer_name() -> String {
    vf_sync::computer_name()
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
        // Several servers (PA-36): B2 sync runs on the main server only.
        if matches!(op, Op::Push(_) | Op::Pull) && !crate::server::runs_here(root) {
            self.error = Some(format!(
                "B2 sync runs on {} for this cabinet (Admin › Devices).",
                crate::server::main_server(root)
                    .map(|m| m.computer)
                    .unwrap_or_else(|| "another server".into())
            ));
            return;
        }
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
        vf_sync::push::set_upload_cap(cfg.upload_mbps);
        self.recall(env.root, data_dir(catalog), cfg, env.device_id);
        if let Some(rx) = &self.counting {
            match rx.try_recv() {
                Ok((pending, conflicts, st)) => {
                    self.counting = None;
                    self.pending = pending;
                    self.conflicts = conflicts;
                    if let Some((rev, p, l)) = st {
                        self.head_rev = rev as u64;
                        self.last_push = p;
                        self.last_pull = l;
                    }
                }
                Err(TryRecvError::Disconnected) => self.counting = None,
                Err(TryRecvError::Empty) => {}
            }
        }
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
        // The pending count reads every document: only while sync is unlocked (Browse's pill
        // and this tab's tables are hidden otherwise).
        if self.session.is_some()
            && self
                .refreshed
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(3))
        {
            self.refresh(ctx, env, catalog);
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
            // Several servers: only the main server pushes (PA-36).
            if cfg.in_window(hour) && crate::server::runs_here(env.root) {
                self.start(ctx, catalog, env.root, Op::Push(None));
            }
        }
    }

    /// Counts again on a thread with its own catalog connection (the pending count reads
    /// every document: ~900K in a large cabinet, PA-34); [`SyncTab::tick`] picks them up.
    fn refresh(&mut self, ctx: &egui::Context, env: &Env, catalog: &Path) {
        self.refreshed = Some(Instant::now());
        if self.counting.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (catalog, device, ctx) = (
            catalog.to_path_buf(),
            env.device_id.to_string(),
            ctx.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("sync-counts".into())
            .spawn(move || {
                let Ok(conn) = vf_core::db::open(&catalog, Storage::Local) else {
                    return;
                };
                let pending = vf_sync::catalog::pending(&conn).unwrap_or_default();
                let conflicts = vf_sync::restore::conflicts(&conn).unwrap_or_default();
                let st = conn
                    .query_row(
                        "SELECT last_manifest_rev, last_push, last_pull FROM sync_state WHERE device_id = ?1",
                        [&device],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .ok();
                let _ = tx.send((pending, conflicts, st));
                ctx.request_repaint();
            });
        if spawned.is_ok() {
            self.counting = Some(rx);
        }
    }

    fn finished(&mut self, d: Done, cfg: &SyncConfig) {
        match d {
            Done::Setup(firm, s, sealed) => {
                let cab = s.cabinet_id.clone();
                self.session = Some(s);
                if let Some(w) = self.apply_remember(&self.setup_data.clone(), &cab, Some(&firm)) {
                    self.error = Some(w);
                }
                let mut c = self.setup_cfg.take().unwrap_or_else(|| cfg.clone());
                c.app_key_sealed = sealed;
                if let Some(w) = self.apply_share(&cab, &firm, &c) {
                    self.error = Some(w);
                }
                self.config_changed = Some(c);
                self.notice =
                    Some("Sync is set up. The first push sends the whole cabinet.".into());
            }
            Done::Unlocked(firm, s) => {
                let cab = s.cabinet_id.clone();
                self.session = Some(s);
                self.locked_by_user = false;
                // Both run: a remember warning never skips the sharing asked for.
                let remember = self.apply_remember(&self.setup_data.clone(), &cab, Some(&firm));
                let share = self.apply_share(&cab, &firm, cfg);
                let warn = match (remember, share) {
                    (Some(a), Some(b)) => Some(format!("{a} {b}")),
                    (a, b) => a.or(b),
                };
                self.notice = Some(if self.remembered {
                    "Sync unlocked, and remembered on this PC for your Windows login.".into()
                } else {
                    "Sync unlocked for this session.".into()
                });
                if warn.is_some() {
                    self.error = warn;
                }
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

    /// The session from keys (no network: the sealed application key is local).
    fn session_from(
        keys: vf_sync::Keys,
        cfg: &SyncConfig,
        device_id: &str,
    ) -> Result<Sync, String> {
        Sync::connect(
            keys,
            &cfg.s3(),
            &cfg.app_key_sealed,
            &cfg.cabinet_id,
            device_id,
        )
        .map_err(|e| e.to_string())
    }

    /// Unlocks with the key this Windows login remembered on this PC, when there is one. Tried
    /// again every 30 s while locked (a share that answers late, sync set up meanwhile), never
    /// after the person locked it.
    fn recall(&mut self, root: &Path, data: &Path, cfg: &SyncConfig, device_id: &str) {
        if self.session.is_some()
            || self.locked_by_user
            || self
                .recall_at
                .is_some_and(|t| t.elapsed() < Duration::from_secs(30))
        {
            return;
        }
        self.recall_at = Some(Instant::now());
        let Ok(cab) = vf_fs::cabinet::read(root) else {
            return;
        };
        // The key the firm keeps in the cabinet for everyone (Q-43): a login or PC that never
        // set sync up takes the bucket settings from it too.
        let shared = vf_sync::setup::shared(root, &cab.cabinet_id);
        self.shared = shared.is_some();
        if let Some((s, keys)) = shared {
            let mut c = cfg.clone();
            if !cfg.configured_for(&cab.cabinet_id) {
                c = SyncConfig {
                    endpoint: s.endpoint.clone(),
                    region: s.region.clone(),
                    bucket: s.bucket.clone(),
                    key_id: s.key_id.clone(),
                    app_key_sealed: s.app_key_sealed.clone(),
                    cabinet_id: cab.cabinet_id.clone(),
                    // Pushing on a timer stays a choice per PC: by hand until set here.
                    auto_minutes: 0,
                    ..SyncConfig::default()
                };
                self.config_changed = Some(c.clone());
            }
            match Self::session_from(keys, &c, device_id) {
                Ok(sess) => {
                    self.session = Some(sess);
                    return;
                }
                Err(e) => tracing::warn!(
                    kind = crate::logging::kind(&e),
                    "shared sync key did not open sync"
                ),
            }
        }
        if !cfg.configured_for(&cab.cabinet_id) {
            return;
        }
        self.remembered = vf_sync::setup::remembered_file(data, &cab.cabinet_id).exists();
        if let Some(keys) = vf_sync::setup::recall(data, &cab.cabinet_id) {
            match Self::session_from(keys, cfg, device_id) {
                Ok(s) => self.session = Some(s),
                // A key from an earlier set-up: forget it, the passphrase opens the new one.
                Err(e) => {
                    tracing::warn!(
                        kind = crate::logging::kind(&e),
                        "remembered sync key is stale"
                    );
                    vf_sync::setup::forget(data, &cab.cabinet_id);
                    self.remembered = false;
                }
            }
        }
    }

    /// Keeps the key in the cabinet for everyone when the box is ticked (Q-43). Unticked leaves
    /// a key someone else shared alone ("Stop sharing the key" removes it).
    fn apply_share(
        &mut self,
        cabinet_id: &str,
        firm: &[u8; 32],
        cfg: &SyncConfig,
    ) -> Option<String> {
        if !self.form.share {
            return None;
        }
        match vf_sync::setup::share(
            &self.setup_root,
            cabinet_id,
            firm,
            &cfg.s3(),
            &cfg.app_key_sealed,
        ) {
            Ok(()) => {
                self.shared = true;
                None
            }
            Err(e) => Some(format!(
                "Sync is unlocked, but the key could not be kept in the cabinet for everyone ({e})."
            )),
        }
    }

    /// Remembers or forgets the key on this PC as the checkbox says. Best effort: sync stays
    /// unlocked either way; a failure is a warning.
    fn apply_remember(
        &mut self,
        data: &Path,
        cabinet_id: &str,
        firm: Option<&[u8; 32]>,
    ) -> Option<String> {
        match (self.form.remember, firm) {
            (true, Some(firm)) => match vf_sync::setup::remember(data, cabinet_id, firm) {
                Ok(()) => {
                    self.remembered = true;
                    None
                }
                Err(e) => Some(format!(
                    "Sync is unlocked, but the key could not be remembered on this PC ({e}): you will be asked for the passphrase next time."
                )),
            },
            (true, None) => None,
            (false, _) => {
                vf_sync::setup::forget(data, cabinet_id);
                self.remembered = false;
                None
            }
        }
    }

    /// Opens sync with the passphrase; remembers (or forgets) the key on this PC as asked.
    /// Unlocks with the typed passphrase. The key derivation (Argon2, about a second) runs on
    /// its own thread, so the window keeps answering (QA S15); the result comes back as
    /// [`Done::Unlocked`].
    fn unlock(
        &mut self,
        ctx: &egui::Context,
        root: &Path,
        data: &Path,
        cfg: &SyncConfig,
        device_id: &str,
    ) {
        // One job at a time: a second click would replace the running one (QA2 U8).
        if self.running() {
            return;
        }
        let pass = zeroize::Zeroizing::new(std::mem::take(&mut self.form.unlock));
        self.setup_data = data.to_path_buf();
        self.setup_root = root.to_path_buf();
        self.error = None;
        let (tx, rx) = std::sync::mpsc::channel();
        let (root, cfg, device) = (root.to_path_buf(), cfg.clone(), device_id.to_string());
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("b2-unlock".into())
            .spawn(move || {
                let r = (|| -> Result<Done, String> {
                    // Wiped wherever it ends up (QA3 #16).
                    let firm = Zeroizing::new(
                        vf_sync::setup::firm_key_local(&root, &cfg.cabinet_id, &pass)
                            .map_err(|e| e.to_string())?,
                    );
                    let keys = vf_sync::Keys::derive(vf_sync::setup::KEY_ID, &firm);
                    Ok(Done::Unlocked(
                        firm,
                        Self::session_from(keys, &cfg, &device)?,
                    ))
                })();
                let _ = tx.send(r);
                ctx.request_repaint();
            })
            .ok();
        self.job = Some(("Unlocking…".into(), rx));
    }

    fn setup(
        &mut self,
        ctx: &egui::Context,
        root: &Path,
        data: &Path,
        device_id: &str,
        cabinet_id: &str,
    ) {
        self.setup_data = data.to_path_buf();
        self.setup_root = root.to_path_buf();
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
        self.setup_cfg = Some(cfg.clone());
        std::thread::Builder::new()
            .name("b2-setup".into())
            .spawn(move || {
                let r = (|| -> Result<Done, String> {
                    let store =
                        vf_sync::Remote::s3(&cfg.s3(), &app_key).map_err(|e| e.to_string())?;
                    let remote = vf_sync::Remote::new(store, &cab);
                    let (firm, keys) = vf_sync::setup::init_with_firm(
                        &root,
                        &remote,
                        &pass,
                        vf_sync::setup::DEFAULT_KDF,
                    )
                    .map_err(|e| e.to_string())?;
                    let sealed = vf_sync::setup::seal_secret(&keys, &app_key);
                    let firm = Zeroizing::new(firm);
                    Ok(Done::Setup(
                        firm,
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

/// Held while a push or pull runs on this catalog: on a server whose sessions share one
/// catalog, one session syncs it at a time (PA-34 review).
fn sync_lock(catalog: &Path) -> Result<std::fs::File, String> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(data_dir(catalog).join("sync.lock"))
        .map_err(|_| {
            "Another session on this computer is syncing this cabinet now; its changes and yours go up together next time.".to_string()
        })
}

fn run(catalog: &Path, root: &Path, s: &Sync, op: Op) -> Result<Done, String> {
    let e = |e: vf_sync::SyncError| e.to_string();
    let _one_at_a_time = match op {
        Op::Push(_) | Op::Pull => Some(sync_lock(catalog)?),
        _ => None,
    };
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
    /// The settings may change: no admin password, or it was given this session (PA-24).
    /// Push, pull and verify do not need it.
    pub admin: bool,
}

const ADMIN_NEEDED: &str = "Needs the admin password (Admin tab).";

pub fn show(ui: &mut egui::Ui, t: &mut SyncTab, env: &mut Env, v: View<'_>) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.heading("Sync");
        ui.label(widgets::caption(
            "Two-way sync of this cabinet with your Backblaze B2 bucket. Everything is encrypted on \
             this PC before it leaves; the bucket's names reveal no clients. The sync passphrase is \
             the only way to the key: keep it with the firm's recovery papers.",
        ));
        // PA-39 (Q-76): allowed, but sync reads every file, so the main PC downloads the whole
        // cloud cabinet.
        if let Some(svc) = crate::cloudmode::cabinet_provider(env.root) {
            ui.label(
                RichText::new(format!(
                    "This cabinet is in {svc}. B2 sync reads every file to send it, so the PC that                      runs sync downloads the whole cabinet from {svc}. Use one or the other."
                ))
                .color(theme::error()),
            );
        }
        ui.add_space(6.0);
        if let Some((what, _)) = &t.job {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(what.as_str());
            });
        }
        if let Some(n) = &t.notice {
            ui.label(RichText::new(n).color(theme::ink()));
        }
        if let Some(e) = &t.error {
            ui.label(RichText::new(e).color(crate::ui::theme::error()));
        }
        ui.add_space(6.0);
        let configured = v.cfg.configured_for(v.cabinet_id);
        if !v.admin {
            // Without the admin password nothing here shares or forgets the key.
            t.form.share = false;
        }
        if !configured && !v.admin {
            ui.label("Sync is not set up on this PC.");
            ui.label(widgets::caption(
                "Setting it up needs the admin password (Admin tab). Once the firm keeps the key in \
                 the cabinet (Sync › \"Let everyone who opens this cabinet sync\"), this PC syncs \
                 without any set-up.",
            ));
        } else if !configured {
            setup_form(ui, t, env, &v);
        } else if t.session.is_none() {
            ui.label("Sync is set up for this cabinet. Unlock it for this session:");
            field(ui, "Sync passphrase", &mut t.form.unlock, true, "");
            ui.checkbox(&mut t.form.remember, "Remember on this PC for my Windows login")
                .on_hover_text(REMEMBER_HELP);
            if v.admin {
                ui.checkbox(&mut t.form.share, SHARE_LABEL)
                    .on_hover_text(SHARE_HELP);
            }
            if ui
                .add_enabled(!t.running(), widgets::primary_button("Unlock"))
                .clicked()
            {
                t.unlock(ui.ctx(), env.root, data_dir(v.catalog), v.cfg, env.device_id);
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
    ui.checkbox(&mut f.remember, "Remember on this PC for my Windows login")
        .on_hover_text(REMEMBER_HELP);
    ui.checkbox(&mut f.share, SHARE_LABEL)
        .on_hover_text(SHARE_HELP);
    let ready = !f.endpoint.trim().is_empty()
        && !f.bucket.trim().is_empty()
        && !f.key_id.trim().is_empty()
        && !f.app_key.is_empty()
        && !f.pass.is_empty();
    if ui
        .add_enabled(ready && !t.running(), widgets::primary_button("Connect"))
        .clicked()
    {
        t.setup(
            ui.ctx(),
            env.root,
            data_dir(v.catalog),
            env.device_id,
            v.cabinet_id,
        );
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
            .add_enabled(
                v.admin,
                egui::Button::new(if cfg.paused { "Resume" } else { "Pause" }),
            )
            .on_disabled_hover_text(ADMIN_NEEDED)
            .clicked()
        {
            cfg.paused = !cfg.paused;
            t.config_changed = Some(cfg);
        }
        if ui
            .add_enabled(
                idle && v.admin,
                egui::Button::new("Restore cabinet to a folder…"),
            )
            .on_disabled_hover_text(ADMIN_NEEDED)
            .on_hover_text("Rebuilds the whole cabinet from the bucket into an empty folder.")
            .clicked()
            && let Some(p) = rfd::FileDialog::new().pick_folder()
        {
            t.start(ui.ctx(), v.catalog, env.root, Op::Restore(p));
        }
        if ui
            .add_enabled(!t.running(), egui::Button::new("Lock"))
            .on_disabled_hover_text("Wait for the running sync to finish.")
            .on_hover_text("Close sync until unlocked again (a remembered key unlocks it at the next start).")
            .clicked()
        {
            t.session = None;
            t.locked_by_user = true;
        }
        let data = data_dir(v.catalog);
        if t.remembered
            && v.admin
            && ui
                .button("Forget on this PC")
                .on_hover_text("Sync will ask for the passphrase again on this PC.")
                .clicked()
        {
            vf_sync::setup::forget(data, v.cabinet_id);
            t.remembered = false;
            t.notice = Some("The key is no longer remembered on this PC.".into());
        }
        if t.shared
            && v.admin
            && ui
                .button("Stop sharing the key")
                .on_hover_text(
                    "Removes the key kept in the cabinet: everyone needs the passphrase (or their own remembered key) again.",
                )
                .clicked()
        {
            vf_sync::setup::unshare(env.root);
            t.shared = false;
            // Not shared again by the next unlock of this session unless ticked again.
            t.form.share = false;
            t.notice = Some("The key is no longer kept in the cabinet for everyone.".into());
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
    if !v.admin {
        ui.label(widgets::caption(ADMIN_NEEDED));
    }
    ui.add_enabled_ui(v.admin, |ui| settings_section(ui, t, v));
}

/// Timers, window and Object Lock (admin only, PA-24).
fn settings_section(ui: &mut egui::Ui, t: &mut SyncTab, v: &View<'_>) {
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
        let l = ui.label("Upload at most (megabits a second, 0 = no limit)");
        changed |= ui
            .add(egui::DragValue::new(&mut cfg.upload_mbps).range(0..=10_000))
            .labelled_by(l.id)
            .on_hover_text(
                "A first upload of a large cabinet can take days; a limit leaves the office line usable meanwhile. This PC only.",
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    /// PA-20: the checkbox decides; unticked forgets a key remembered earlier.
    #[test]
    fn remember_box_remembers_and_unticked_forgets() {
        let t = tempfile::tempdir().unwrap();
        let mut tab = SyncTab::default();
        let firm = [9u8; 32];
        assert!(tab.form.remember, "ticked by default");
        assert_eq!(tab.apply_remember(t.path(), "cab-1", Some(&firm)), None);
        assert!(tab.remembered);
        assert!(vf_sync::setup::recall(t.path(), "cab-1").is_some());
        tab.form.remember = false;
        tab.apply_remember(t.path(), "cab-1", Some(&firm));
        assert!(!tab.remembered);
        assert!(vf_sync::setup::recall(t.path(), "cab-1").is_none());
    }
    /// Q-43: a key the firm keeps in the cabinet opens sync for a login that never set it up,
    /// with the bucket settings, pushing by hand until that PC chooses a timer.
    #[test]
    fn shared_key_opens_sync_for_everyone() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        let o = crate::session::create(
            &t.path().join("pc1"),
            "dev-1",
            &crate::session::CreateRequest {
                root: &root,
                name: "Firm",
            },
        )
        .unwrap();
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        drop(o);
        let firm = [5u8; 32];
        let keys = vf_sync::Keys::derive(vf_sync::setup::KEY_ID, &firm);
        let sealed = vf_sync::setup::seal_secret(&keys, "invented application key");
        let cfg = SyncConfig {
            endpoint: "https://s3.example.invalid".into(),
            region: "us-west-004".into(),
            bucket: "invented-bucket".into(),
            key_id: "invented-key-id".into(),
            app_key_sealed: sealed,
            cabinet_id: cab.clone(),
            ..SyncConfig::default()
        };
        let mut sharer = SyncTab {
            setup_root: root.clone(),
            ..Default::default()
        };
        assert!(sharer.form.share, "ticked by default (owner 2026-11-04)");
        assert_eq!(sharer.apply_share(&cab, &firm, &cfg), None);

        // Another login: no settings of its own.
        let mut other = SyncTab::default();
        other.recall(
            &root,
            &t.path().join("pc2"),
            &SyncConfig::default(),
            "dev-2",
        );
        assert!(other.session.is_some(), "{:?}", other.error);
        assert!(other.shared);
        let got = other
            .config_changed
            .expect("settings taken from the cabinet");
        assert_eq!(got.bucket, "invented-bucket");
        assert_eq!(got.auto_minutes, 0);

        // Stop sharing: the next login is asked again.
        vf_sync::setup::unshare(&root);
        let mut third = SyncTab::default();
        third.recall(
            &root,
            &t.path().join("pc3"),
            &SyncConfig::default(),
            "dev-3",
        );
        assert!(third.session.is_none());
    }
}
