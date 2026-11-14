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
use vf_fs::clients::{B2Rule, B2Years};
use vf_sync::Sync;
use vf_sync::restore::{Conflict, Keep};

use crate::config::SyncConfig;
use crate::ui::browse::Env;
use crate::ui::widgets::UiExt as _;
use crate::ui::{theme, widgets};

/// Documents downloaded and checked by Verify now.
pub const VERIFY_SAMPLE: usize = 20;

pub enum Done {
    /// (firm key, session, sealed application key).
    Setup(Zeroizing<[u8; 32]>, Sync, String),
    /// (firm key, session): unlocked with the passphrase.
    Unlocked(Zeroizing<[u8; 32]>, Sync),
    Push(vf_sync::push::PushReport),
    /// The documents selected in Browse, pushed by hand (PA-62): the report and how many.
    PushDocs(vf_sync::push::PushReport, usize),
    Pull(vf_sync::pull::PullReport),
    Verify(usize, usize),
    Restored(vf_sync::restore::RestoreReport, PathBuf),
    Exported(usize, PathBuf),
}

/// What runs on the sync thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Push(Option<String>),
    /// These documents (catalog ids), now, whatever the B2 rules say (PA-62).
    PushDocs(Vec<String>),
    /// These clients (catalog ids), one after the other (PA-63: the ticked clients).
    PushClients(Vec<String>),
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

/// Which clients the Sync page's table lists (PA-63).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ClientShow {
    /// Those with something to send (as the list before the table was).
    #[default]
    Pending,
    All,
    Always,
    Never,
}

/// A B2 rule in the editor (PA-63).
struct RuleEdit {
    /// Its place in the list; `None`: a new rule.
    at: Option<usize>,
    rule: B2Rule,
    /// Which tax years: 0 any, 1 this year and some before, 2 none.
    years: u8,
    /// The number of years before this one, as typed.
    back: String,
}

impl RuleEdit {
    fn of(at: Option<usize>, rule: B2Rule) -> Self {
        let (years, back) = match rule.years {
            B2Years::Any => (0, "3".to_string()),
            B2Years::Back(n) => (1, n.to_string()),
            B2Years::None => (2, "3".to_string()),
        };
        RuleEdit {
            at,
            rule,
            years,
            back,
        }
    }
}

/// What a rule sends, in words.
fn rule_summary(r: &B2Rule, types: &[(String, String)], this_year: i32) -> String {
    let folders = if r.folders.is_empty() {
        "any folder".to_string()
    } else {
        r.folders.join(", ")
    };
    let doc_types = if r.doc_types.is_empty() {
        "any doc type".to_string()
    } else {
        r.doc_types
            .iter()
            .map(|id| {
                types
                    .iter()
                    .find(|(i, _)| i == id)
                    .map_or("(a doc type no longer listed)", |(_, n)| n.as_str())
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let years = match (r.years, r.no_year) {
        (B2Years::Any, true) => "any tax year, or none".to_string(),
        (B2Years::Any, false) => "any tax year".to_string(),
        (B2Years::None, true) => "no tax year only".to_string(),
        (B2Years::None, false) => "nothing (no tax years chosen)".to_string(),
        (B2Years::Back(n), no) => format!(
            "tax year {} or later{}",
            this_year - i32::from(n),
            if no { ", or none" } else { "" }
        ),
    };
    format!("{folders} · {doc_types} · {years}")
}

/// Ticks or unticks `item` in `list` (any letter case).
fn tick(ui: &mut egui::Ui, list: &mut Vec<String>, item: &str, label: &str) {
    let mut on = list.iter().any(|x| x.eq_ignore_ascii_case(item));
    if ui.checkbox(&mut on, label).changed() {
        list.retain(|x| !x.eq_ignore_ascii_case(item));
        if on {
            list.push(item.to_string());
        }
    }
}

#[derive(Default)]
pub struct SyncTab {
    /// The cabinet is still being listed behind the window (set by the app, PA-60).
    pub hold: bool,
    pub session: Option<Sync>,
    job: Option<(String, Receiver<Result<Done, String>>)>,
    /// The client a running push or pull is for (`Some("")` = all).
    pub running_for: Option<String>,
    /// Pending documents per client id (`""` = the cabinet), refreshed every few seconds.
    pub pending: BTreeMap<String, usize>,
    /// Documents the B2 rule holds back now (count, bytes), counted with `pending` (PA-53).
    pub held: Option<(usize, u64)>,
    /// The cabinet's B2 rules as last read, when, and whether a list was ever saved (PA-63).
    /// The file is on the share: read every 30 s and after a save, not every frame.
    b2_rules: Option<(Instant, Vec<B2Rule>, bool)>,
    /// What a rule can name, read with the rules: doc types (id, name) and the cabinet
    /// template's folders below a client folder.
    rule_choices: (Vec<(String, String)>, Vec<String>),
    /// The rule being added or changed.
    rule_edit: Option<RuleEdit>,
    /// Each client's own Always send / Never send (client id → "always" / "never"), read with
    /// the rules.
    b2_choices: BTreeMap<String, String>,
    /// The client table (PA-63): the words typed in Find a client.
    client_find: String,
    /// Which clients it lists.
    client_show: ClientShow,
    /// Most pending first, instead of by name.
    client_by_pending: bool,
    /// Ticked clients (ids), for the buttons above the table.
    ticked: std::collections::BTreeSet<String>,
    /// The table's rows (client ids, `""` = the cabinet) in the order shown, made again only
    /// when what it shows changes: thousands of clients are not sorted on every repaint.
    table: Vec<String>,
    /// What `table` was made for: the find words, the choice of clients, the order, how many
    /// clients there are and how many have something pending, and the client asked for from
    /// Browse.
    table_for: Option<(String, ClientShow, bool, usize, usize, Option<String>)>,
    /// The pending counts or the clients' choices changed since `table` was made.
    table_stale: bool,
    /// Client id → name, made with `table`.
    names: std::collections::HashMap<String, String>,
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
    Option<(usize, u64)>,
);

const SHARE_LABEL: &str = "Let everyone who opens this cabinet sync without the passphrase";
const SHARE_HELP: &str = "Keeps the sync key and the bucket settings in the cabinet's _Vibe\\keys folder, which is never uploaded: the Backblaze copy stays encrypted, and anyone who can open the cabinet on the network can sync. Keep the passphrase with the firm's recovery papers.";

const REMEMBER_HELP: &str = "The key is sealed with Windows (DPAPI) for your login on this \
computer and kept in this PC's data folder: sync unlocks itself when you start Vibe Files. Another \
login or another computer cannot open it. The passphrase stays the way back in.";

/// This PC's data folder (the catalog's).
/// The bucket settings the firm keeps in the cabinet with its shared key (Q-43), for a login
/// or PC that never set sync up.
pub(crate) fn config_from_shared(s: &vf_sync::setup::Shared, cabinet_id: &str) -> SyncConfig {
    SyncConfig {
        endpoint: s.endpoint.clone(),
        region: s.region.clone(),
        bucket: s.bucket.clone(),
        key_id: s.key_id.clone(),
        app_key_sealed: s.app_key_sealed.clone(),
        cabinet_id: cabinet_id.to_string(),
        // Pushing on a timer stays a choice per PC: by hand until set here.
        auto_minutes: 0,
        ..SyncConfig::default()
    }
}

/// Where this Windows login's remembered sync key is kept (Q-38): the catalog's folder; beside
/// a server's shared catalog, a folder of the login's own, since every login of the server uses
/// that catalog and one login's key must not replace another's (PA-40: the scheduled push's
/// login keeps its key).
pub(crate) fn key_dir(catalog: &Path) -> PathBuf {
    let dir = data_dir(catalog);
    let shared = crate::shared::shared_root().is_some_and(|r| dir.starts_with(r));
    match vf_core::windows_login() {
        Some(login) if shared => dir.join("logins").join(login_folder(&login)),
        _ => dir.to_path_buf(),
    }
}

/// A login's own folder name: readable, and unique (`john.smith` and `john_smith`, or a name
/// with no ASCII letters, never share one).
fn login_folder(login: &str) -> String {
    let readable: String = vf_fs::changes::safe_id(&login.to_lowercase())
        .chars()
        .take(32)
        .collect();
    let tag = &vf_sync::crypto::sha256(login.to_lowercase().as_bytes())[..12];
    format!("{readable}-{tag}")
}

/// Makes a login's own key folder readable and writable by that login (and the system and
/// administrators) only: the shared catalog's folder lets every login of the server write, and
/// another login must not delete this one's key. Best effort; the key itself is sealed with
/// Windows for this login either way.
fn protect_login_folder(dir: &Path) {
    use std::os::windows::process::CommandExt;
    let (Ok(user), Some(_)) = (std::env::var("USERNAME"), legacy_dir(dir)) else {
        return;
    };
    let me = match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
        _ => user,
    };
    let r = std::process::Command::new("icacls")
        .arg(dir)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("{me}:(OI)(CI)F"))
        .args([
            "/grant:r",
            "*S-1-5-18:(OI)(CI)F",
            "/grant:r",
            "*S-1-5-32-544:(OI)(CI)F",
        ])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output();
    if !r.as_ref().is_ok_and(|o| o.status.success()) {
        tracing::warn!("sync key folder left with the catalog folder's permissions");
    }
}

/// The folder a login's key was kept in before PA-40, when `dir` is a login's own.
fn legacy_dir(dir: &Path) -> Option<&Path> {
    let parent = dir.parent()?;
    if parent.file_name()? == "logins" {
        parent.parent()
    } else {
        None
    }
}

/// This login's remembered key: its own, else one it left beside the shared catalog earlier
/// (another login's there does not open for it).
pub(crate) fn recall_key(dir: &Path, cabinet_id: &str) -> Option<vf_sync::Keys> {
    vf_sync::setup::recall(dir, cabinet_id)
        .or_else(|| vf_sync::setup::recall(legacy_dir(dir)?, cabinet_id))
}

/// Forgets this login's remembered key, and an earlier copy beside the shared catalog only when
/// it is this login's.
pub(crate) fn forget_key(dir: &Path, cabinet_id: &str) {
    vf_sync::setup::forget(dir, cabinet_id);
    if let Some(old) = legacy_dir(dir)
        && vf_sync::setup::recall(old, cabinet_id).is_some()
    {
        vf_sync::setup::forget(old, cabinet_id);
    }
}

/// The folder of `catalog` (this computer's sync lock is there).
pub(crate) fn data_dir(catalog: &Path) -> &Path {
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
        // The cabinet is still being listed behind the window (PA-60): only the documents
        // chosen go up (a share pushes them first); the rest waits for the whole catalog.
        if self.hold && !matches!(op, Op::PushDocs(_)) {
            self.error = Some(
                "The cabinet's files are still being listed. Try again when that is done.".into(),
            );
            return;
        }
        if self.running() {
            return;
        }
        self.error = None;
        // Several servers (PA-36): the whole cabinet syncs on the main computer only. Any
        // other computer may push one client (PA-41), bringing in that client's changes only.
        let main_here = crate::server::runs_here(root);
        // A cloud cabinet: each PC has its own copy of the folder, synced late perhaps, so a
        // client is not pushed from one that is not the main PC.
        let cloud = !crate::server::server_mode() && crate::cloudmode::is_cloud(root);
        if !main_here
            && cloud
            && matches!(
                op,
                Op::Push(_) | Op::PushDocs(_) | Op::PushClients(_) | Op::Pull
            )
        {
            self.error = Some(format!(
                "B2 sync runs on {} for this cabinet (Admin › Devices).",
                crate::server::main_server(root)
                    .map(|m| m.computer)
                    .unwrap_or_else(|| "a main PC not chosen yet".into())
            ));
            return;
        }
        if matches!(op, Op::Push(None) | Op::Pull) && !main_here {
            self.error = Some(format!(
                "The whole cabinet syncs on {} (Admin › Devices). From here, push one client: Push this client now in Browse, or Push beside it below.",
                crate::server::main_server(root)
                    .map(|m| m.computer)
                    .unwrap_or_else(|| "another computer".into())
            ));
            return;
        }
        let alone = !main_here;
        self.running_for = match &op {
            Op::Push(c) => Some(c.clone().unwrap_or_default()),
            Op::PushDocs(_) | Op::PushClients(_) | Op::Pull => Some(String::new()),
            _ => None,
        };
        let label = match &op {
            Op::Push(Some(_)) => "Pushing this client…",
            Op::PushDocs(_) => "Pushing the selected documents…",
            Op::PushClients(_) => "Pushing the ticked clients…",
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
                let r = run(&catalog, &root, &s, op, alone);
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
        self.recall(env.root, &key_dir(catalog), cfg, env.device_id);
        if let Some(rx) = &self.counting {
            match rx.try_recv() {
                Ok((pending, conflicts, st, held)) => {
                    self.counting = None;
                    self.pending = pending;
                    self.table_stale = true;
                    if held.is_some() {
                        self.held = held;
                    }
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
            && !self.hold
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
        // The held count only when something changed it (a push, the rule): QA 2026-11-12.
        let (root, want_held) = (env.root.to_path_buf(), self.held.is_none());
        let spawned = std::thread::Builder::new()
            .name("sync-counts".into())
            .spawn(move || {
                let Ok(conn) = vf_core::db::open(&catalog, Storage::Local) else {
                    return;
                };
                // Read here, not on the window's thread (the file is on the share).
                let rules = vf_sync::rules::Rules::load_or_default(&root);
                let pending = vf_sync::catalog::pending(&conn, &rules).unwrap_or_default();
                let held = want_held
                    .then(|| vf_sync::catalog::held(&conn, &rules).ok())
                    .flatten();
                let conflicts = vf_sync::restore::conflicts(&conn).unwrap_or_default();
                let st = conn
                    .query_row(
                        "SELECT last_manifest_rev, last_push, last_pull FROM sync_state WHERE device_id = ?1",
                        [&device],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .ok();
                let _ = tx.send((pending, conflicts, st, held));
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
                // What is held back may have changed (sent, or new files): counted again.
                self.held = None;
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
            Done::PushDocs(r, n) => {
                self.held = None;
                if r.pulled.as_ref().is_some_and(|p| p.changed() > 0) {
                    self.changed = true;
                }
                self.notice = Some(format!(
                    "Pushed the {} selected to B2 ({} new objects, {}). {} from now on, like anything else in B2.",
                    if n == 1 {
                        "document".to_string()
                    } else {
                        format!("{n} documents")
                    },
                    r.objects,
                    widgets::size(r.bytes as i64),
                    if n == 1 {
                        "It keeps syncing"
                    } else {
                        "They keep syncing"
                    },
                ));
                if r.skipped > 0 {
                    self.error = Some(
                        "A selected file could not be read just now, so its client was not sent: try again.".into(),
                    );
                }
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
                c = config_from_shared(&s, &cab.cabinet_id);
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
        let recalled = recall_key(data, &cab.cabinet_id);
        self.remembered =
            recalled.is_some() || vf_sync::setup::remembered_file(data, &cab.cabinet_id).exists();
        if let Some(keys) = recalled {
            match Self::session_from(keys, cfg, device_id) {
                Ok(s) => self.session = Some(s),
                // A key from an earlier set-up: forget it, the passphrase opens the new one.
                Err(e) => {
                    tracing::warn!(
                        kind = crate::logging::kind(&e),
                        "remembered sync key is stale"
                    );
                    forget_key(data, &cab.cabinet_id);
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
                    protect_login_folder(data);
                    self.remembered = true;
                    None
                }
                Err(e) => Some(format!(
                    "Sync is unlocked, but the key could not be remembered on this PC ({e}): you will be asked for the passphrase next time."
                )),
            },
            (true, None) => None,
            (false, _) => {
                forget_key(data, cabinet_id);
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

/// Pushes these documents to B2 now, on the calling thread (a portal share runs it first:
/// owner 2026-11-13, Q-103). The same push as Browse's "Push the selected documents now".
pub fn push_docs_now(catalog: &Path, root: &Path, s: &Sync, ids: &[String]) -> Result<(), String> {
    let main_here = crate::server::runs_here(root);
    let cloud = !crate::server::server_mode() && crate::cloudmode::is_cloud(root);
    if !main_here && cloud {
        return Err(format!(
            "B2 sync runs on {} for this cabinet (Admin › Devices)",
            crate::server::main_server(root)
                .map(|m| m.computer)
                .unwrap_or_else(|| "a main PC not chosen yet".into())
        ));
    }
    match run(catalog, root, s, Op::PushDocs(ids.to_vec()), !main_here)? {
        Done::PushDocs(r, _) if r.skipped > 0 => {
            Err("a file could not be read just now; try again".into())
        }
        _ => Ok(()),
    }
}

/// Held while a push or pull runs on this catalog: on a server whose sessions share one
/// catalog, one session syncs it at a time (PA-34 review).
pub(crate) fn sync_lock(catalog: &Path) -> Result<std::fs::File, String> {
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

/// `alone`: this computer is not the cabinet's main one, so a client push brings in and sends
/// that client only (PA-41).
fn run(catalog: &Path, root: &Path, s: &Sync, op: Op, alone: bool) -> Result<Done, String> {
    let e = |e: vf_sync::SyncError| e.to_string();
    let _one_at_a_time = match op {
        Op::Push(_) | Op::PushDocs(_) | Op::PushClients(_) | Op::Pull => Some(sync_lock(catalog)?),
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
        Op::Push(Some(client)) if alone => {
            let mut conn = caught_up(catalog, root, s, alone)?;
            // The servers working in this same folder (server mode, on the share): a segment
            // one of them sent is taken as it is, nothing written into the folder (PA-41). A
            // PC or laptop with its own copy is not one of them.
            let same_folder: Vec<String> = vf_fs::safeguards::devices(root)
                .into_iter()
                .filter(|d| d.server)
                .map(|d| d.name)
                .collect();
            vf_sync::push::push_client(&mut conn, root, s, &client, &same_folder)
                .map(Done::Push)
                .map_err(e)
        }
        Op::PushClients(clients) => {
            let mut conn = caught_up(catalog, root, s, alone)?;
            let same_folder: Vec<String> = vf_fs::safeguards::devices(root)
                .into_iter()
                .filter(|d| d.server)
                .map(|d| d.name)
                .collect();
            // Each client as a client pushed alone; one that fails stops the rest, with what
            // went before it already in B2.
            let mut total = vf_sync::push::PushReport::default();
            for c in &clients {
                let r = if alone {
                    vf_sync::push::push_client(&mut conn, root, s, c, &same_folder)
                } else {
                    vf_sync::push::push(&mut conn, root, s, Some(c))
                }
                .map_err(e)?;
                total.objects += r.objects;
                total.bytes += r.bytes;
                total.segments += r.segments;
                total.documents += r.documents;
                total.skipped += r.skipped;
                total.head_rev = total.head_rev.max(r.head_rev);
                if r.pulled.is_some() {
                    total.pulled = r.pulled;
                }
            }
            Ok(Done::Push(total))
        }
        Op::PushDocs(ids) => {
            let mut conn = caught_up(catalog, root, s, alone)?;
            // As a client pushed alone (PA-41): the servers working in this same folder.
            let same_folder: Vec<String> = vf_fs::safeguards::devices(root)
                .into_iter()
                .filter(|d| d.server)
                .map(|d| d.name)
                .collect();
            let n = ids.len();
            vf_sync::push::push_docs(&mut conn, root, s, &ids, &same_folder)
                .map(|r| Done::PushDocs(r, n))
                .map_err(e)
        }
        Op::Push(only) => {
            let mut conn = caught_up(catalog, root, s, alone)?;
            let r = vf_sync::push::push(&mut conn, root, s, only.as_deref()).map_err(e)?;
            // The whole cabinet went up: every computer's startup check sees when (PA-40). Not
            // when files were left (unreadable now): those are not in B2 yet.
            if only.is_none() && r.skipped == 0 {
                crate::lastpush::record(root);
            }
            Ok(Done::Push(r))
        }
        Op::Pull => {
            let mut conn = caught_up(catalog, root, s, alone)?;
            vf_sync::pull::pull(&mut conn, root, s)
                .map(Done::Pull)
                .map_err(e)
        }
    }
}

/// The catalog, with the other computers' changes to the cabinet read first on several servers
/// or a cloud cabinet (their change logs): a push sends the folder as it is, and a pull finds
/// a file another computer filed here and sent as already here (PA-41).
fn caught_up(
    catalog: &Path,
    root: &Path,
    s: &Sync,
    alone: bool,
) -> Result<vf_core::db::Connection, String> {
    let mut conn = vf_core::db::open(catalog, Storage::Local).map_err(|e| e.to_string())?;
    if alone || crate::server::server_mode() || crate::cloudmode::is_cloud(root) {
        let writer = crate::server::writer_id(&conn);
        vf_fs::changes::catch_up(&mut conn, root, &writer, &s.device_id)?;
    }
    Ok(conn)
}

fn tempfile_catalog(catalog: &Path) -> Result<PathBuf, String> {
    let dir = catalog.parent().ok_or("no data folder")?.join("tmp");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("restore-{}.db", vf_core::new_id())))
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, password: bool, hint: &str) {
    ui.horizontal(|ui| {
        // Left-aligned in a fixed column, as the app's other forms (UI review 2026-11-11).
        let l = ui
            .allocate_ui_with_layout(
                egui::vec2(150.0, 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(150.0);
                    ui.label(label)
                },
            )
            .inner;
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
        egui::ScrollArea::vertical()
            .id_salt("sync-page")
            .auto_shrink([false, false])
            .show(ui, |ui| page(ui, t, env, v));
    });
}

fn page(ui: &mut egui::Ui, t: &mut SyncTab, env: &mut Env, v: View<'_>) {
    {
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
                    "This cabinet is in {svc}. B2 sync reads every file to send it, so the PC that runs sync downloads the whole cabinet from {svc}. Use one or the other."
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
            ui.checkbox(
                &mut t.form.remember,
                "Remember on this PC for my Windows login",
            )
            .on_hover_text(REMEMBER_HELP);
            if v.admin {
                ui.checkbox(&mut t.form.share, SHARE_LABEL)
                    .on_hover_text(SHARE_HELP);
            }
            if ui
                .add_enabled(!t.running(), widgets::primary_button("Unlock"))
                .clicked()
            {
                t.unlock(
                    ui.ctx(),
                    env.root,
                    &key_dir(v.catalog),
                    v.cfg,
                    env.device_id,
                );
            }
        } else {
            unlocked(ui, t, env, &v);
        }
    }
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
            &key_dir(v.catalog),
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
        let data = key_dir(v.catalog);
        if t.remembered
            && v.admin
            && ui
                .button("Forget on this PC")
                .on_hover_text("Sync will ask for the passphrase again on this PC.")
                .clicked()
        {
            forget_key(&data, v.cabinet_id);
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

    // What goes to B2 and the settings above the client list (owner 2026-11-12: thousands of
    // clients hid them).
    b2_rule_section(ui, t, env, v.admin);

    ui.add_space(10.0);
    ui.label(RichText::new("Settings").strong());
    if !v.admin {
        ui.label(widgets::caption(ADMIN_NEEDED));
    }
    ui.add_enabled_ui(v.admin, |ui| settings_section(ui, t, v));

    ui.add_space(10.0);
    client_table(ui, t, env, v, idle);
}

/// The clients (PA-63, owner 2026-11-13: thousands of clients in one list with no way to find
/// one, and everything one client at a time): found by name, shown by what they have waiting
/// or by their own setting, ticked, and acted on together.
fn client_table(ui: &mut egui::Ui, t: &mut SyncTab, env: &mut Env, v: &View<'_>, idle: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Clients").strong());
        if let Some(c) = &t.filter_client {
            let name = t.names.get(c).map_or("?", String::as_str);
            ui.label(widgets::caption(format!("showing {name}")));
            if ui.small_btn("Show all").clicked() {
                t.filter_client = None;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        let l = ui.label("Find a client");
        ui.add(
            egui::TextEdit::singleline(&mut t.client_find)
                .hint_text("part of a name")
                .desired_width(200.0),
        )
        .labelled_by(l.id);
        ui.separator();
        ui.label("Show");
        ui.radio_value(
            &mut t.client_show,
            ClientShow::Pending,
            "With something to send",
        );
        ui.radio_value(&mut t.client_show, ClientShow::All, "All clients");
        ui.radio_value(&mut t.client_show, ClientShow::Always, "Always send");
        ui.radio_value(&mut t.client_show, ClientShow::Never, "Never send");
        ui.separator();
        ui.checkbox(&mut t.client_by_pending, "Most pending first");
    });

    // The rows, made again only when what they show changed.
    let key = (
        t.client_find.trim().to_lowercase(),
        t.client_show,
        t.client_by_pending,
        v.clients.len(),
        t.pending.len(),
        t.filter_client.clone(),
    );
    if t.table_stale || t.table_for.as_ref() != Some(&key) {
        t.names = v.clients.iter().cloned().collect();
        let find = &key.0;
        let mut rows: Vec<(&str, &str)> = v
            .clients
            .iter()
            .map(|(id, name)| (id.as_str(), name.as_str()))
            .filter(|(id, name)| {
                t.filter_client.as_ref().is_none_or(|f| f == id)
                    && (find.is_empty() || name.to_lowercase().contains(find.as_str()))
                    && match t.client_show {
                        ClientShow::Pending => t.pending.contains_key(*id),
                        ClientShow::All => true,
                        ClientShow::Always => {
                            t.b2_choices.get(*id).map(String::as_str) == Some("always")
                        }
                        ClientShow::Never => {
                            t.b2_choices.get(*id).map(String::as_str) == Some("never")
                        }
                    }
            })
            .collect();
        if t.client_by_pending {
            rows.sort_by(|a, b| {
                let n = |id: &str| t.pending.get(id).copied().unwrap_or(0);
                n(b.0)
                    .cmp(&n(a.0))
                    .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
            });
        } else {
            rows.sort_by_cached_key(|(_, name)| name.to_lowercase());
        }
        let mut table: Vec<String> = rows.into_iter().map(|(id, _)| id.to_string()).collect();
        // What is not in a client folder, first, when it has something to send.
        if t.client_show == ClientShow::Pending
            && t.pending.contains_key("")
            && find.is_empty()
            && t.filter_client.is_none()
        {
            table.insert(0, String::new());
        }
        // A pending client this catalog has no name for (it was just removed) is still listed.
        if t.client_show == ClientShow::Pending && find.is_empty() {
            for id in t.pending.keys() {
                if !id.is_empty()
                    && !t.names.contains_key(id)
                    && t.filter_client.as_ref().is_none_or(|f| f == id)
                {
                    table.push(id.clone());
                }
            }
        }
        t.table = table;
        t.table_for = Some(key);
        t.table_stale = false;
        // A tick on a client no longer listed would be acted on unseen.
        let shown: std::collections::HashSet<&str> = t.table.iter().map(String::as_str).collect();
        t.ticked.retain(|id| shown.contains(id.as_str()));
    }

    // The buttons for the ticked clients.
    let mut go: Option<Op> = None;
    let mut choice: Option<Option<&str>> = None;
    ui.horizontal_wrapped(|ui| {
        let n = t.ticked.len();
        ui.label(match n {
            0 => format!("{} listed · none ticked", t.table.len()),
            n => format!("{} listed · {n} ticked", t.table.len()),
        });
        if ui.small_btn("Tick all listed").clicked() {
            t.ticked = t
                .table
                .iter()
                .filter(|id| !id.is_empty())
                .cloned()
                .collect();
        }
        if ui
            .add_enabled(n > 0, egui::Button::new("Untick all").small())
            .clicked()
        {
            t.ticked.clear();
        }
        ui.separator();
        if ui
            .add_enabled(n > 0 && idle, egui::Button::new("Push ticked now"))
            .on_hover_text(
                "Sends what the rules let through for each ticked client, one after the other.",
            )
            .clicked()
        {
            go = Some(Op::PushClients(t.ticked.iter().cloned().collect()));
        }
        ui.separator();
        ui.label("Set ticked to");
        if ui
            .add_enabled(n > 0, egui::Button::new("Always send"))
            .on_hover_text("Every document of these clients goes to B2.")
            .clicked()
        {
            choice = Some(Some("always"));
        }
        if ui
            .add_enabled(n > 0, egui::Button::new("Never send"))
            .on_hover_text(
                "Nothing more of these clients goes to B2 by itself; what is there keeps syncing.",
            )
            .clicked()
        {
            choice = Some(Some("never"));
        }
        if ui
            .add_enabled(n > 0, egui::Button::new("Use the rules"))
            .on_hover_text("These clients follow the firm's rules again.")
            .clicked()
        {
            choice = Some(None);
        }
    });
    if let Some(c) = choice {
        let ids: Vec<String> = t.ticked.iter().cloned().collect();
        match vf_fs::clients::set_b2_choices(env.root, &ids, c) {
            Ok(()) => {
                t.notice = Some(format!(
                    "{} {} set to {}.",
                    ids.len(),
                    if ids.len() == 1 { "client" } else { "clients" },
                    match c {
                        Some("always") => "Always send",
                        Some(_) => "Never send",
                        None => "the firm's rules",
                    }
                ));
                t.error = None;
                for id in &ids {
                    match c {
                        Some(c) => {
                            t.b2_choices.insert(id.clone(), c.to_string());
                        }
                        None => {
                            t.b2_choices.remove(id);
                        }
                    }
                }
                // Counted again with the new choices.
                t.held = None;
                t.refreshed = None;
                t.table_stale = true;
            }
            Err(e) => t.error = Some(format!("Not saved: {e}")),
        }
    }

    // Its own scrolling box, laying out only the rows in view (thousands of clients).
    let row_h =
        ui.spacing().interact_size.y.max(
            ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y,
        ) + ui.spacing().item_spacing.y;
    let rows = t.table.clone();
    egui::ScrollArea::vertical()
        .id_salt("sync-pending-scroll")
        .max_height(360.0)
        .auto_shrink([false, true])
        .show_rows(ui, row_h, rows.len(), |ui, range| {
            egui::Grid::new("sync-pending")
                .striped(true)
                .num_columns(5)
                .min_col_width(60.0)
                .show(ui, |ui| {
                    for id in &rows[range] {
                        if id.is_empty() {
                            ui.label("");
                            ui.label("Cabinet (not in a client folder, master files)");
                        } else {
                            let name = t.names.get(id).map_or("(client)", String::as_str);
                            let mut on = t.ticked.contains(id);
                            if ui
                                .checkbox(&mut on, "")
                                .on_hover_text(format!("Tick {name}"))
                                .changed()
                            {
                                if on {
                                    t.ticked.insert(id.clone());
                                } else {
                                    t.ticked.remove(id);
                                }
                            }
                            ui.label(name);
                        }
                        match t.pending.get(id) {
                            Some(n) => ui.label(format!("{n} pending")),
                            None => ui.label(widgets::caption("nothing to send")),
                        };
                        ui.label(match t.b2_choices.get(id).map(String::as_str) {
                            Some("always") => "Always send",
                            Some("never") => "Never send",
                            _ => "",
                        });
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
        });
    if t.table.is_empty() {
        ui.label(widgets::caption(match t.client_show {
            ClientShow::Pending if t.client_find.trim().is_empty() => {
                "Nothing waiting: this PC and the bucket agree."
            }
            _ => "No client matches.",
        }));
    }
    if let Some(op) = go {
        t.start(ui.ctx(), v.catalog, env.root, op);
    }
}

/// What goes to B2 (owner 2026-11-12, PA-53; a list of rules since PA-63): a document never
/// sent goes up when any rule matches; anything already in B2 keeps syncing. Shared by every PC
/// (`clients.vf.json`); changed with the admin password, like the other settings.
fn b2_rule_section(ui: &mut egui::Ui, t: &mut SyncTab, env: &mut Env, admin: bool) {
    ui.label(RichText::new("What goes to B2").strong());
    if t.b2_rules
        .as_ref()
        .is_none_or(|(at, ..)| at.elapsed() > Duration::from_secs(30))
    {
        // One small file (never the whole client list), and the names a rule can use.
        let b2 = vf_fs::clients::load_b2(env.root).unwrap_or_default();
        let saved = b2.rules.is_some();
        t.b2_rules = Some((Instant::now(), vf_sync::rules::listed(&b2), saved));
        if t.b2_choices != b2.clients {
            t.b2_choices = b2.clients.clone();
            t.table_stale = true;
        }
        let types: Vec<(String, String)> = env
            .conn
            .prepare(
                "SELECT id, name FROM doc_type WHERE deleted_at IS NULL
                 ORDER BY name COLLATE NOCASE",
            )
            .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .unwrap_or_default();
        let (tree, _, _) =
            filer_core::template::load_or_seed(&vf_fs::clients::master_dir(env.root));
        // The year folders of the template are not folders to name: the rule has its own years.
        let folders = filer_core::template::folder_templates(&tree)
            .into_iter()
            .filter(|f| !f.contains('{'))
            .collect();
        t.rule_choices = (types, folders);
    }
    let (rules, saved) = t
        .b2_rules
        .as_ref()
        .map(|(_, r, s)| (r.clone(), *s))
        .unwrap_or_default();
    let this_year = chrono::Datelike::year(&chrono::Local::now());
    ui.label(widgets::caption(
        "A document not yet in B2 goes there when any rule below matches it. Anything already in B2 keeps syncing (edits, moves, deletions). A client's own Always send / Never send wins, and a document pushed or shared by hand goes whatever the rules say.",
    ));
    if !saved {
        ui.label(widgets::caption(
            "These rules are the one rule used until now, written as a list. The list is saved the first time you change it.",
        ));
    }
    if rules.is_empty() {
        ui.label(
            RichText::new("No rules: nothing goes to B2 by itself.").color(theme::ink_secondary()),
        );
    }
    // What a button asked for: a rule to edit (or a new one), or the list to save.
    let mut edit: Option<RuleEdit> = None;
    let mut save: Option<(Vec<B2Rule>, String)> = None;
    let idle = admin && t.rule_edit.is_none();
    for (i, r) in rules.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            let name = if r.name.trim().is_empty() {
                format!("Rule {}", i + 1)
            } else {
                r.name.clone()
            };
            ui.label(RichText::new(&name).strong());
            ui.label(rule_summary(r, &t.rule_choices.0, this_year));
            ui.add_enabled_ui(idle, |ui| {
                if ui
                    .small_btn("Edit")
                    .on_hover_text(format!("Change the rule {name}"))
                    .clicked()
                {
                    edit = Some(RuleEdit::of(Some(i), r.clone()));
                }
                if ui
                    .small_btn("Delete")
                    .on_hover_text(format!(
                        "Take the rule {name} off the list. What it sent stays in B2."
                    ))
                    .clicked()
                {
                    let mut left = rules.clone();
                    left.remove(i);
                    save = Some((left, format!("The rule {name} is deleted.")));
                }
            });
        });
    }
    if ui
        .add_enabled(idle, egui::Button::new("Add rule"))
        .clicked()
    {
        edit = Some(RuleEdit::of(
            None,
            B2Rule {
                years: B2Years::Back(vf_sync::rules::DEFAULT_YEARS_BACK),
                ..B2Rule::default()
            },
        ));
    }
    if edit.is_some() {
        t.rule_edit = edit;
    }
    if let Some(e) = &mut t.rule_edit {
        let mut done = false;
        let mut cancel = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                RichText::new(if e.at.is_some() {
                    "Change the rule"
                } else {
                    "New rule"
                })
                .strong(),
            );
            ui.horizontal(|ui| {
                let l = ui.label("Rule name");
                ui.add(egui::TextEdit::singleline(&mut e.rule.name).desired_width(240.0))
                    .labelled_by(l.id);
            });
            ui.label(widgets::caption(
                "Folders (in a client folder, at any depth). None ticked: any folder.",
            ));
            ui.horizontal_wrapped(|ui| {
                for f in &t.rule_choices.1 {
                    tick(ui, &mut e.rule.folders, f, f);
                }
                // A folder the rule names that the template no longer has stays listed.
                for f in e.rule.folders.clone() {
                    if !t.rule_choices.1.iter().any(|x| x.eq_ignore_ascii_case(&f)) {
                        tick(
                            ui,
                            &mut e.rule.folders,
                            &f,
                            &format!("{f} (not in the template)"),
                        );
                    }
                }
            });
            ui.label(widgets::caption("Doc types. None ticked: any doc type."));
            egui::ScrollArea::vertical()
                .id_salt("b2-rule-types")
                .max_height(120.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (id, name) in &t.rule_choices.0 {
                            tick(ui, &mut e.rule.doc_types, id, name);
                        }
                    });
                });
            ui.horizontal(|ui| {
                ui.label("Tax years:");
                ui.radio_value(&mut e.years, 0, "Any tax year");
                ui.radio_value(&mut e.years, 1, "This year and");
                let l = ui.label("years before it");
                ui.add_enabled(
                    e.years == 1,
                    egui::TextEdit::singleline(&mut e.back).desired_width(30.0),
                )
                .labelled_by(l.id);
                ui.radio_value(&mut e.years, 2, "None with a tax year");
            });
            ui.checkbox(&mut e.rule.no_year, "Include documents with no tax year");
            ui.horizontal(|ui| {
                done = ui.add(widgets::primary_button("Save rule")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if cancel {
            t.rule_edit = None;
        } else if done {
            let years = match e.years {
                0 => Some(B2Years::Any),
                2 => Some(B2Years::None),
                _ => e
                    .back
                    .trim()
                    .parse::<u16>()
                    .ok()
                    .filter(|n| *n <= 100)
                    .map(B2Years::Back),
            };
            match years {
                None => t.error = Some("Type a number of years from 0 to 100.".into()),
                Some(B2Years::None) if !e.rule.no_year => {
                    t.error = Some(
                        "This rule would send nothing: choose tax years, or tick Include documents with no tax year.".into(),
                    );
                }
                Some(y) => {
                    let mut rule = e.rule.clone();
                    rule.name = rule.name.trim().to_string();
                    rule.years = y;
                    let mut list = rules.clone();
                    let what = match e.at {
                        Some(i) if i < list.len() => {
                            list[i] = rule;
                            "The rule is changed."
                        }
                        _ => {
                            list.push(rule);
                            "The rule is added."
                        }
                    };
                    save = Some((list, what.to_string()));
                }
            }
        }
    }
    if let Some((list, what)) = save {
        match vf_fs::clients::set_b2_rules(env.root, &list) {
            Ok(()) => {
                t.notice = Some(format!(
                    "{what} What it holds back or lets through is counted again below."
                ));
                t.error = None;
                t.held = None;
                t.refreshed = None;
                t.b2_rules = None;
                t.rule_edit = None;
            }
            Err(e) => t.error = Some(format!("Not saved: {e}")),
        }
    }
    if !admin {
        ui.label(widgets::caption(ADMIN_NEEDED));
    }
    match t.held {
        Some((0, _)) => {
            ui.label(widgets::caption("Nothing is held back."));
        }
        Some((n, bytes)) => {
            ui.label(
                RichText::new(format!(
                    "Held back from B2 now: {} documents ({}). They are not in B2: make sure the share is backed up another way.",
                    n,
                    widgets::size(bytes as i64)
                ))
                .color(theme::warn()),
            );
        }
        None => {
            ui.label(widgets::caption("Counting what is held back…"));
        }
    }
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

    /// PA-41: a computer that is not the main one pushes one client: what another server
    /// filed there (its change log) is read first and goes up; the other client stays pending.
    #[test]
    fn a_client_pushed_off_the_main_computer_takes_other_servers_filings() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Clients");
        let data = tmp.path().join("pc1");
        let mut o = crate::session::create(
            &data,
            "dev-1",
            &crate::session::CreateRequest {
                root: &root,
                name: "Firm",
            },
        )
        .unwrap();
        for (id, name) in [("1042", "Anderson Farms LLC"), ("2002", "Bell Ranch Inc")] {
            let p = vf_fs::newclient::plan(
                &o.conn,
                &root,
                &vf_fs::newclient::NewClient {
                    partner: String::new(),
                    ext_id: id.into(),
                    name: name.into(),
                    entity_type: None,
                },
            )
            .unwrap();
            vf_fs::newclient::create(&mut o.conn, &root, &p, "dev-1").unwrap();
        }
        let put = |rel: &str, body: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
            p
        };
        put(
            "Anderson Farms LLC (1042)/Income Tax/2025/w2.pdf",
            "invented w2",
        );
        put("Bell Ranch Inc (2002)/Payroll/2025/941.pdf", "invented 941");
        vf_fs::rebuild::rebuild(&mut o.conn, &root, "dev-1").unwrap();
        let w = crate::server::writer_id(&o.conn);
        vf_fs::changes::mark_start(&o.conn, &root, &w).unwrap();
        let anderson: String = o
            .conn
            .query_row("SELECT id FROM client WHERE ext_id = '1042'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let catalog = o.catalog.clone();
        drop(o);
        // Another server files a K-1 for Anderson.
        let k1 = put(
            "Anderson Farms LLC (1042)/Income Tax/2025/k1.pdf",
            "invented k1",
        );
        vf_fs::changes::set_writer(&root, "server-b-1234");
        vf_fs::changes::note_paths([k1.as_path()]);
        vf_fs::changes::clear_writer();

        let cab = vf_fs::cabinet::read(&root).unwrap();
        let remote = vf_sync::Remote::new(vf_sync::Remote::memory(), &cab.cabinet_id);
        let keys = vf_sync::setup::init(
            &root,
            &remote,
            "correct horse battery staple",
            vf_sync::setup::DEFAULT_KDF,
        )
        .unwrap();
        let s = Sync {
            remote,
            keys,
            cabinet_id: cab.cabinet_id.clone(),
            device_id: "dev-1".into(),
            device_name: "SERVER-2".into(),
        };
        let Done::Push(r) =
            run(&catalog, &root, &s, Op::Push(Some(anderson.clone())), true).unwrap()
        else {
            panic!("a push")
        };
        assert_eq!(r.documents, 2, "{r:?}");
        let conn = vf_core::db::open(&catalog, Storage::Local).unwrap();
        let pending =
            vf_sync::catalog::pending(&conn, &vf_sync::rules::Rules::load_or_default(&root))
                .unwrap();
        assert!(!pending.contains_key(&anderson));
        assert_eq!(pending.len(), 1, "Bell Ranch still waits: {pending:?}");
        // One client is not the whole cabinet: no B2 backup time recorded.
        assert!(crate::lastpush::read(&root).is_none());
        // PA-62: one Bell Ranch document pushed by hand from this same computer goes up; the
        // sync lock is free again after each run.
        let doc: String = conn
            .query_row(
                "SELECT id FROM document WHERE rel_path LIKE '%941.pdf'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        drop(conn);
        let Done::PushDocs(r, n) =
            run(&catalog, &root, &s, Op::PushDocs(vec![doc.clone()]), true).unwrap()
        else {
            panic!("a push of documents")
        };
        assert_eq!((n, r.objects, r.segments), (1, 1, 1), "{r:?}");
        let conn = vf_core::db::open(&catalog, Storage::Local).unwrap();
        assert!(vf_sync::catalog::sent_ids(&conn).unwrap().contains(&doc));
    }

    /// QA: login folder names never collide (punctuation, letter case, no ASCII letters).
    #[test]
    fn login_folders_are_distinct() {
        let names = [
            "OFFICE\\john.smith",
            "OFFICE\\john_smith",
            "OFFICE\\\u{5f20}\u{4f1f}",
            "OFFICE\\\u{674e}\u{5a1c}",
        ];
        let folders: std::collections::BTreeSet<String> =
            names.iter().map(|n| login_folder(n)).collect();
        assert_eq!(folders.len(), names.len(), "{folders:?}");
        assert_eq!(login_folder("OFFICE\\Jane"), login_folder("office\\jane"));
    }

    /// PA-40: on a server each login keeps its key in a folder of its own beside the shared
    /// catalog; a key this login left beside the catalog earlier still opens sync, and
    /// forgetting removes both.
    #[test]
    fn each_login_keeps_its_own_key_and_an_old_one_still_works() {
        let t = tempfile::tempdir().unwrap();
        let mine = t.path().join("logins").join("me");
        assert_eq!(legacy_dir(&mine), Some(t.path()));
        assert_eq!(legacy_dir(t.path()), None);
        let firm = [7u8; 32];
        vf_sync::setup::remember(t.path(), "cab-1", &firm).unwrap();
        assert!(recall_key(&mine, "cab-1").is_some(), "the earlier copy");
        vf_sync::setup::remember(&mine, "cab-1", &firm).unwrap();
        forget_key(&mine, "cab-1");
        assert!(recall_key(&mine, "cab-1").is_none());
        assert!(!vf_sync::setup::remembered_file(t.path(), "cab-1").exists());
    }

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
