//! Several Remote Desktop servers on one cabinet (PA-36, Q-70/Q-71/Q-73). Vibe Files runs from a
//! network path (`\\server\share\APPS\Vibe Files`); the cabinet is on a file share; people log
//! into any of several session hosts. Then ("server mode"):
//!
//! - no data folder on the share: each login uses `%LOCALAPPDATA%\Vibe Files\data` on the
//!   server it is on (settings in the old data folder beside the exe are copied over once);
//! - each server keeps one catalog for all its logins (`%PROGRAMDATA%`, made on first open),
//!   started from a copy another server left in `<cabinet>\_Vibe\catalog-seed\` (or, the
//!   first time, from the old catalog beside the exe), then checked by size and date;
//! - every change is noted in the cabinet's change logs (`vf_fs::changes`) and the other
//!   servers re-read the folders named there (on open, every 15 minutes, on a client's open);
//! - text recognition (Tesseract and the text layers it writes) and B2 sync run on one server,
//!   the main server (`<cabinet>\_Vibe\main-server.json`); the others index the text layers.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use vf_core::db::{Connection, OpenFlags};

use crate::shared::CATALOG_FILE;

/// Whether `p` is on another machine: a UNC path, or a drive Windows reports as remote.
pub fn is_network(p: &Path) -> bool {
    let s = p.to_string_lossy();
    if s.starts_with(r"\\?\UNC\") || (s.starts_with(r"\\") && !s.starts_with(r"\\?\")) {
        return true;
    }
    let letter = s.trim_start_matches(r"\\?\");
    if letter.len() >= 2 && letter.as_bytes()[1] == b':' {
        use windows::Win32::Storage::FileSystem::GetDriveTypeW;
        use windows::core::HSTRING;
        let drive = HSTRING::from(format!("{}\\", &letter[..2]));
        // SAFETY: a valid, NUL-terminated wide string for the call's duration.
        let kind = unsafe { GetDriveTypeW(&drive) };
        return kind == 4; // DRIVE_REMOTE
    }
    false
}

/// Server mode: a Remote Desktop server (Windows Server) running Vibe Files from a network
/// path. An office PC running it from the same folder keeps its usual data folder and
/// catalog (owner 2026-11-05). `VIBE_FILES_SERVER=1` or `0` decides instead.
pub fn server_mode() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| match std::env::var("VIBE_FILES_SERVER").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => match mode_setting() {
            Setting::On => true,
            Setting::Off => false,
            Setting::Auto => auto_detected(),
        },
    })
}

/// What Windows says: a Windows Server running Vibe Files from a network folder.
pub fn auto_detected() -> bool {
    is_windows_server() && vf_core::exe_dir().is_some_and(|d| is_network(&d))
}

/// This computer's server-mode choice (Admin › Devices › Servers, owner 2026-11-05), kept in
/// `%PROGRAMDATA%\Vibe Files\server-mode.txt`: it is read before the data folder is chosen,
/// and it is the computer's, not one login's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Auto,
    On,
    Off,
}

fn setting_path() -> Option<PathBuf> {
    std::env::var_os("PROGRAMDATA")
        .map(PathBuf::from)
        .map(|p| p.join("Vibe Files").join("server-mode.txt"))
}

pub fn mode_setting() -> Setting {
    match setting_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .as_deref()
        .map(str::trim)
    {
        Some("on") => Setting::On,
        Some("off") => Setting::Off,
        _ => Setting::Auto,
    }
}

/// Saves this computer's choice; takes effect at the next start of Vibe Files.
pub fn set_mode_setting(s: Setting) -> Result<()> {
    let p = setting_path().context("this computer has no ProgramData folder")?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    let v = match s {
        Setting::Auto => "auto",
        Setting::On => "on",
        Setting::Off => "off",
    };
    std::fs::write(&p, v).with_context(|| format!("writing {}", p.display()))
}

/// Whether this is Windows Server (where Remote Desktop session hosts run), from
/// `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\InstallationType`.
pub fn is_windows_server() -> bool {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::HSTRING;
    let key = HSTRING::from(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let name = HSTRING::from("InstallationType");
    let mut buf = [0u16; 64];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: valid wide strings and a buffer of `len` bytes for the call's duration.
    let ok = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &key,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if ok.is_err() {
        return false;
    }
    let n = (len as usize / 2).saturating_sub(1).min(buf.len());
    // "Server", "Server Core" (and any other Server kind); not "Client".
    String::from_utf16_lossy(&buf[..n])
        .to_ascii_lowercase()
        .starts_with("server")
}

/// This computer's name.
pub fn computer() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

/// Where a server leaves its catalog for the others to start from.
pub fn seed_path(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR)
        .join("catalog-seed")
        .join(CATALOG_FILE)
}

/// `local_state` keys a catalog copy must not carry to another server.
const NOT_COPIED: &[&str] = &[
    "sync_device_id",
    "rebuild_unfinished",
    "catalog_writer_id",
    "health_check_month",
    "ocr_reads_owner",
];

/// A consistent copy of the catalog at `from` (read only) into `to`, for another server: its
/// interrupted-change journal and this-server-only state left out.
fn copy_catalog(from: &Path, to: &Path) -> Result<()> {
    let src = Connection::open_with_flags(from, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening {}", from.display()))?;
    let _ = std::fs::remove_file(to);
    src.execute("VACUUM INTO ?1", [to.to_string_lossy().as_ref()])
        .with_context(|| format!("copying {}", from.display()))?;
    drop(src);
    let c = Connection::open(to)?;
    c.execute("DELETE FROM tx_journal", [])?;
    for k in NOT_COPIED {
        c.execute("DELETE FROM local_state WHERE k = ?1", [k])?;
    }
    Ok(())
}

/// This server's catalog for the cabinet: the shared one if there is one; else one made now in
/// ProgramData, started from the cabinet's seed copy or the old catalog beside the exe when one
/// belongs to this cabinet (else empty: the open indexes the cabinet). `legacy` is that old
/// catalog.
pub fn provision(cabinet_id: &str, root: &Path, legacy: Option<&Path>) -> Result<PathBuf> {
    let base = crate::shared::shared_root().context("this server has no ProgramData folder")?;
    provision_in(&base, cabinet_id, root, legacy)
}

fn provision_in(
    base: &Path,
    cabinet_id: &str,
    root: &Path,
    legacy: Option<&Path>,
) -> Result<PathBuf> {
    let dir = crate::shared::prepare_dir(base, cabinet_id)?;
    let target = dir.join(CATALOG_FILE);
    if target.is_file() {
        return Ok(target);
    }
    // One login of the server makes it; the others wait for it.
    let _one = lock(&dir.join("provision.lock"))?;
    if target.is_file() {
        return Ok(target);
    }
    let part = dir.join(format!("{CATALOG_FILE}.part"));
    let seed = seed_path(root);
    for (from, is_seed) in [(Some(seed.as_path()), true), (legacy, false)] {
        let Some(from) = from.filter(|f| f.is_file()) else {
            continue;
        };
        match copy_catalog(from, &part) {
            Ok(()) if belongs(&part, cabinet_id) => {
                tracing::info!(seed = is_seed, "server catalog started from a copy");
                if !is_seed {
                    // The old catalog beside the exe was not kept up to date by the change
                    // logs: the first open re-indexes the cabinet by size and date.
                    let c = Connection::open(&part)?;
                    c.execute(
                        "INSERT INTO local_state (k, v) VALUES ('rebuild_unfinished', 'rebuild')
                         ON CONFLICT(k) DO UPDATE SET v = excluded.v",
                        [],
                    )?;
                }
                std::fs::rename(&part, &target)?;
                return Ok(target);
            }
            Ok(()) => {}
            Err(e) => tracing::warn!(
                kind = crate::logging::kind(&e.to_string()),
                "catalog copy not used"
            ),
        }
        let _ = std::fs::remove_file(&part);
    }
    Ok(target)
}

/// Whether the catalog at `p` is this cabinet's.
fn belongs(p: &Path, cabinet_id: &str) -> bool {
    Connection::open_with_flags(p, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .and_then(|c| {
            c.query_row(
                "SELECT count(*) FROM cabinet WHERE id = ?1",
                [cabinet_id],
                |r| r.get::<_, i64>(0),
            )
        })
        .is_ok_and(|n| n > 0)
}

/// A lock file held with no sharing, waited for up to 30 minutes.
fn lock(p: &Path) -> Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    let start = std::time::Instant::now();
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(p)
        {
            Ok(f) => return Ok(f),
            // Long enough for another login copying a seed of a few GB over the network.
            Err(e) if start.elapsed() > std::time::Duration::from_secs(30 * 60) => {
                return Err(e).with_context(|| format!("waiting for {}", p.display()));
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(250)),
        }
    }
}

/// This catalog's id in the cabinet's change logs (made once, kept in the catalog).
pub fn writer_id(conn: &Connection) -> String {
    let _ = conn.execute(
        "INSERT OR IGNORE INTO local_state (k, v) VALUES ('catalog_writer_id', ?1)",
        [format!("{}-{}", computer(), &vf_core::new_id()[..8])],
    );
    conn.query_row(
        "SELECT v FROM local_state WHERE k = 'catalog_writer_id'",
        [],
        |r| r.get(0),
    )
    .unwrap_or_default()
}

/// Leaves a copy of this server's catalog for the others to start from, when there is none or
/// it is a week old. The copy has read the change logs as far as this catalog had, its own
/// included. Run on a thread (the copy is 1-3 GB for ~900K documents).
pub fn publish_seed(catalog: &Path, root: &Path, writer: &str) -> Result<bool> {
    let seed = seed_path(root);
    let fresh = std::fs::metadata(&seed)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < std::time::Duration::from_secs(7 * 86_400));
    if fresh {
        return Ok(false);
    }
    let dir = seed.parent().context("no seed folder")?;
    std::fs::create_dir_all(dir)?;
    let part = dir.join(format!(
        "{CATALOG_FILE}.{}.part",
        vf_fs::changes::safe_id(writer)
    ));
    // This server's own log as far as it reaches before the copy starts: changes noted while
    // the copy runs (minutes for GBs) are read again by whoever starts from it.
    let own = vf_fs::changes::log_len(root, writer);
    copy_catalog(catalog, &part)?;
    {
        let c = Connection::open(&part)?;
        let mut seen = vf_fs::changes::load_seen(&c).unwrap_or_default();
        seen.insert(vf_fs::changes::safe_id(writer), own);
        vf_fs::changes::save_seen(&c, &seen).map_err(anyhow::Error::msg)?;
    }
    std::fs::rename(&part, &seed)?;
    Ok(true)
}

/// The server text recognition and B2 sync run on.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MainServer {
    pub computer: String,
    pub since: String,
}

fn main_path(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR).join("main-server.json")
}

pub fn main_server(root: &Path) -> Option<MainServer> {
    serde_json::from_slice(&std::fs::read(main_path(root)).ok()?).ok()
}

/// Makes this computer the main server.
pub fn set_main(root: &Path) -> Result<()> {
    let m = MainServer {
        computer: computer(),
        since: vf_core::now_rfc3339(),
    };
    vf_fs::cabinet::write_replace(&main_path(root), &serde_json::to_vec_pretty(&m)?)
        .map_err(anyhow::Error::msg)
}

/// Whether text recognition and B2 sync run here: always on a single PC; in server mode only on
/// the main server (the first server to ask becomes it).
pub fn runs_here(root: &Path) -> bool {
    if !server_mode() {
        return true;
    }
    match main_server(root) {
        Some(m) => m.computer.eq_ignore_ascii_case(&computer()),
        // No main server recorded: the first to make the file takes the role (made only if it
        // is not there, so two servers starting together cannot both take it). A file that
        // cannot be read for a moment (the share, another server writing it) never makes this
        // one main.
        None if !main_path(root).exists() => claim_main(root),
        None => false,
    }
}

/// Makes this computer the main server only if none is recorded.
fn claim_main(root: &Path) -> bool {
    use std::io::Write;
    let m = MainServer {
        computer: computer(),
        since: vf_core::now_rfc3339(),
    };
    let path = main_path(root);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut f) => {
            let ok = serde_json::to_vec_pretty(&m)
                .ok()
                .is_some_and(|b| f.write_all(&b).and_then(|()| f.sync_all()).is_ok());
            if !ok {
                drop(f);
                let _ = std::fs::remove_file(&path);
            }
            ok
        }
        // Another server made it first.
        Err(_) => main_server(root).is_some_and(|m| m.computer.eq_ignore_ascii_case(&computer())),
    }
}

/// The data folder of a login in server mode: never beside the exe on the share. Settings in
/// the old one (`beside`, when this login owns it) are copied over once.
pub fn personal_data(beside: &Path, personal: &Path, login: Option<&str>) -> PathBuf {
    let cfg = personal.join("config.toml");
    let old = beside.join("config.toml");
    let owner = std::fs::read_to_string(beside.join("owner.txt")).unwrap_or_default();
    if !cfg.exists()
        && old.is_file()
        && login.is_some_and(|me| owner.trim().eq_ignore_ascii_case(me))
        && std::fs::create_dir_all(personal).is_ok()
    {
        let _ = std::fs::copy(&old, &cfg);
    }
    personal.to_path_buf()
}

/// Keeps a server's catalog up to date with the others (PA-36): reads their change logs every
/// 15 minutes and when a different client is opened (at most every 30 s), on a thread with
/// its own connection; then leaves a seed copy when the cabinet has none or it is a week old.
pub struct Watch {
    job: Option<std::sync::mpsc::Receiver<Vec<String>>>,
    last: Option<std::time::Instant>,
    client: Option<String>,
}

const EVERY: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const AT_MOST: std::time::Duration = std::time::Duration::from_secs(30);

impl Default for Watch {
    /// Counting from now: opening the cabinet has just caught up.
    fn default() -> Self {
        Watch {
            job: None,
            last: Some(std::time::Instant::now()),
            client: None,
        }
    }
}

impl Watch {
    /// Every frame. True when a catch-up re-read folders (Browse should read the catalog).
    pub fn tick(
        &mut self,
        ctx: &egui::Context,
        catalog: &Path,
        root: &Path,
        writer: &str,
        device_id: &str,
        client: Option<String>,
    ) -> bool {
        if let Some(rx) = &self.job {
            return match rx.try_recv() {
                Ok(folders) => {
                    self.job = None;
                    !folders.is_empty()
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => false,
            };
        }
        let since = self.last.map(|t| t.elapsed());
        let other_client = client.is_some() && client != self.client;
        let due = match since {
            None => true,
            Some(e) => e >= EVERY || (other_client && e >= AT_MOST),
        };
        if !due {
            ctx.request_repaint_after(EVERY);
            return false;
        }
        self.last = Some(std::time::Instant::now());
        self.client = client;
        let (catalog, root, writer, device) = (
            catalog.to_path_buf(),
            root.to_path_buf(),
            writer.to_string(),
            device_id.to_string(),
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let started = std::thread::Builder::new()
            .name("other-servers".into())
            .spawn(move || {
                let mut folders = vec![];
                if let Ok(mut conn) = vf_core::db::open(&catalog, vf_core::db::Storage::Local) {
                    match vf_fs::changes::catch_up(&mut conn, &root, &writer, &device) {
                        Ok(f) => folders = f,
                        Err(e) => tracing::warn!(
                            kind = crate::logging::kind(&e),
                            "other servers' changes not read"
                        ),
                    }
                }
                if let Err(e) = publish_seed(&catalog, &root, &writer) {
                    tracing::warn!(
                        kind = crate::logging::kind(&e.to_string()),
                        "catalog copy for other servers not left"
                    );
                }
                let _ = tx.send(folders);
                ctx.request_repaint();
            });
        if started.is_ok() {
            self.job = Some(rx);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    /// The registry read agrees with Windows' own `reg query` of InstallationType.
    #[test]
    fn server_detection_matches_windows() {
        let out = std::process::Command::new("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
                "/v",
                "InstallationType",
            ])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let server = text
            .lines()
            .any(|l| l.contains("InstallationType") && l.trim_end().ends_with("Server"));
        assert_eq!(is_windows_server(), server, "{text}");
    }

    #[test]
    fn network_paths_are_told_apart() {
        assert!(is_network(Path::new(r"\\xc.lan\THGR18C$\APPS\Vibe Files")));
        assert!(is_network(Path::new(r"\\?\UNC\xc.lan\share\x")));
        assert!(!is_network(Path::new(r"C:\Program Files\Vibe Files")));
        assert!(!is_network(Path::new(r"\\?\C:\x")));
    }

    /// A second server starts from the seed the first left, which knows the first's own log.
    #[test]
    fn a_server_catalog_starts_from_the_seed() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        let a = t.path().join("a.db");
        {
            let c = vf_core::db::open(&a, Storage::Local).unwrap();
            c.execute(
                "INSERT INTO cabinet (id, firm_id, root_path, schema_ver, layout_ver, created)
                 VALUES ('cab-1', 'firm', 'x', 1, 1, 't')",
                [],
            )
            .unwrap();
            c.execute(
                "INSERT INTO local_state (k, v) VALUES ('sync_device_id', 'server-a-sync')",
                [],
            )
            .unwrap();
        }
        // Server A has written to its change log.
        vf_fs::changes::set_writer(&root, "server-a");
        vf_fs::changes::note(&[vf_core::tx::FileOp::Mkdir {
            path: root.join("Client One/x"),
        }]);
        vf_fs::changes::clear_writer();
        assert!(publish_seed(&a, &root, "server-a").unwrap());
        assert!(
            !publish_seed(&a, &root, "server-a").unwrap(),
            "fresh: not again"
        );
        // Server B: from the seed.
        let base = t.path().join("B-ProgramData");
        let b = provision_in(&base, "cab-1", &root, None).unwrap();
        let c = vf_core::db::open(&b, Storage::Local).unwrap();
        let n: i64 = c
            .query_row("SELECT count(*) FROM cabinet WHERE id = 'cab-1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 1);
        let sync: Option<String> = c
            .query_row(
                "SELECT v FROM local_state WHERE k = 'sync_device_id'",
                [],
                |r| r.get(0),
            )
            .ok();
        assert_eq!(sync, None, "server A's own state is not copied");
        // A's change before the seed is not news to B.
        let mut seen = vf_fs::changes::load_seen(&c).unwrap();
        assert!(vf_fs::changes::changed_since(&root, "server-b", &mut seen, true).is_empty());
        // Another cabinet's catalog is not used.
        let other = provision_in(&t.path().join("C"), "cab-2", &root, None).unwrap();
        assert!(
            !other.exists(),
            "nothing to start from: empty, indexed at open"
        );
    }

    /// Code review: the main server is taken only when none is recorded, and a file that
    /// cannot be read never hands this server the role.
    #[test]
    fn the_main_server_is_claimed_once() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        assert!(claim_main(root));
        assert_eq!(main_server(root).unwrap().computer, computer());
        // Someone else's.
        std::fs::write(
            main_path(root),
            r#"{"computer": "OTHER-SERVER", "since": "t"}"#,
        )
        .unwrap();
        assert!(!claim_main(root), "already taken");
        // Unreadable for a moment: not taken over.
        std::fs::write(main_path(root), "{half").unwrap();
        assert!(main_server(root).is_none());
        assert!(main_path(root).exists());
    }

    #[test]
    fn settings_come_over_once_for_their_owner() {
        let t = tempfile::tempdir().unwrap();
        let beside = t.path().join("share").join("data");
        std::fs::create_dir_all(&beside).unwrap();
        std::fs::write(beside.join("owner.txt"), r"XC\kim").unwrap();
        std::fs::write(beside.join("config.toml"), "device_id = \"x\"").unwrap();
        let kim = t.path().join("kim");
        assert_eq!(personal_data(&beside, &kim, Some(r"xc\KIM")), kim);
        assert!(kim.join("config.toml").is_file());
        let pat = t.path().join("pat");
        personal_data(&beside, &pat, Some(r"XC\pat"));
        assert!(!pat.join("config.toml").exists(), "only its owner's");
    }
}
