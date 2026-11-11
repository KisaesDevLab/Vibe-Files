//! Push (P11 step 3): changed documents' contents go up (content-addressed, skipped when the
//! bucket has them, so an interrupted push resumes), then each changed client's segment, then
//! the cabinet segment, then `manifest/HEAD`. B2 has no conditional PUT, so HEAD is written and
//! read back: when another workstation's HEAD is there instead, this push pulls and tries again.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, NoFileOps, Transaction};

use crate::catalog::{self, CABINET};
use crate::crypto::sha256;
use crate::layout::{self, CabinetSegment, ClientSegment, DeviceRecord, Head};
use crate::{Result, Sync, SyncError};

/// Transaction / audit label.
pub const LABEL: &str = "sync_push";
const TRIES: usize = 3;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PushReport {
    pub objects: usize,
    pub bytes: u64,
    pub segments: usize,
    pub documents: usize,
    /// Files that no longer match the catalog (edited outside Vibe Files): left for the
    /// integrity sweep, sent next time.
    pub skipped: usize,
    pub head_rev: u64,
    /// The pull this push ran first.
    pub pulled: Option<crate::pull::PullReport>,
}

/// The cabinet's master files (`_Vibe`-relative path → text), and parser profiles.
pub fn master_files(root: &Path) -> BTreeMap<String, String> {
    let vibe = root.join(vf_fs::cabinet::VIBE_DIR);
    let mut out = BTreeMap::new();
    // Taxpayer-ID columns are stripped from the client list on disk before it is sent.
    let _ = vf_fs::clients::strip_tins_on_disk(root);
    for f in layout::MASTER_FILES {
        if let Ok(t) = std::fs::read_to_string(vibe.join(f)) {
            out.insert((*f).to_string(), t);
        }
    }
    if let Ok(rd) = std::fs::read_dir(vibe.join("parsers")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".toml")
                && let Ok(t) = std::fs::read_to_string(e.path())
            {
                out.insert(format!("parsers/{n}"), t);
            }
        }
    }
    out
}

pub fn files_hashes(files: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    files
        .iter()
        .map(|(k, v)| (k.clone(), sha256(v.as_bytes())))
        .collect()
}

/// Clients sent per round: each round is its own commit of `manifest/HEAD`, so a first push of
/// thousands of clients is saved as it goes and another workstation's push only redoes one
/// round (PA-34).
const ROUND_CLIENTS: usize = 100;
/// Documents per round (a round also ends at this many changed documents).
const ROUND_DOCS: usize = 5_000;
/// Uploads at once.
const UPLOADERS: usize = 8;
/// Files at or above this size are uploaded one at a time (each is held in memory whole while
/// sealed and sent).
const BIG: u64 = 32 * 1024 * 1024;
/// Uploads needing a look at the bucket beyond which the bucket's objects are listed once
/// instead of asked about one by one.
const LIST_ABOVE: usize = 200;

/// Upload cap, bytes a second (0 = none); set from this PC's sync settings.
static UPLOAD_CAP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Caps uploads at `mbps` megabits a second (0 = no cap), so a first upload of a large cabinet
/// leaves the office line usable.
pub fn set_upload_cap(mbps: u32) {
    UPLOAD_CAP.store(
        u64::from(mbps) * 1_000_000 / 8,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// Sends what changed. `only` limits the push to one client (Browse's "Push this client now";
/// the cabinet's master files go with it when they changed). Many changed clients go up in
/// rounds, each committed on its own.
pub fn push(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    only: Option<&str>,
) -> Result<PushReport> {
    push_in_rounds(conn, root, s, only, ROUND_CLIENTS)
}

/// [`push`] with at most `round_clients` clients per round.
pub fn push_in_rounds(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    only: Option<&str>,
    round_clients: usize,
) -> Result<PushReport> {
    let mut rep = PushReport::default();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut listed: Option<Mutex<HashSet<String>>> = None;
    loop {
        let mut attempt = 0;
        let more = loop {
            // Up to date with the bucket first: a push never hides another workstation's work.
            let head = s.head()?.unwrap_or_default();
            if head.rev > catalog::last_rev(conn, &s.device_id)? {
                rep.pulled = Some(crate::pull::pull(conn, root, s)?);
            }
            match push_once(
                conn,
                root,
                s,
                only,
                round_clients.max(1),
                &mut rep,
                &mut done,
                &mut listed,
                None,
            ) {
                Err(SyncError::Raced) if attempt + 1 < TRIES => {
                    attempt += 1;
                    tracing::info!(attempt, "sync push raced another workstation; retrying");
                    continue;
                }
                r => break r?,
            }
        };
        if !more {
            return Ok(rep);
        }
    }
}

/// Pushes one client from a computer that does not run the cabinet's sync (PA-41): only that
/// client's segment is brought in first, never the other clients' (the main computer's pull
/// writes those into the shared folder) nor the cabinet's master files, which are not sent
/// either. `same_folder`: the computers working in this cabinet folder ([`crate::pull`]'s
/// `apply_client`). The manifest rev this computer has seen stays where it was, so its next full
/// exchange still pulls everything, and the bucket is not told it has seen more than it has.
pub fn push_client(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    client: &str,
    same_folder: &[String],
) -> Result<PushReport> {
    let mut rep = PushReport::default();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut listed: Option<Mutex<HashSet<String>>> = None;
    let mut attempt = 0;
    loop {
        let head = s.head()?.unwrap_or_default();
        let mut pulled = crate::pull::PullReport {
            head_rev: head.rev,
            ..Default::default()
        };
        let latest = crate::pull::apply_client(conn, s, &head, client, same_folder, &mut pulled)?;
        if pulled.segments > 0 {
            rep.pulled = Some(pulled);
        }
        // The bucket's entries this catalog has no row for (a filing not read here yet, a
        // tombstone from before this catalog): sent again as they are, never dropped.
        let mut carry: Vec<crate::layout::DocEntry> = vec![];
        for e in latest.map(|seg| seg.documents).unwrap_or_default() {
            let known: bool = conn.query_row(
                "SELECT count(*) > 0 FROM document WHERE id = ?1",
                [&e.doc_id],
                |r| r.get(0),
            )?;
            if !known {
                carry.push(e);
            }
        }
        match push_once(
            conn,
            root,
            s,
            Some(client),
            1,
            &mut rep,
            &mut done,
            &mut listed,
            Some(&carry),
        ) {
            Err(SyncError::Raced) if attempt + 1 < TRIES => {
                attempt += 1;
                tracing::info!(attempt, "sync push raced another workstation; retrying");
            }
            r => {
                r?;
                return Ok(rep);
            }
        }
    }
}

/// One round. `Ok(true)`: clients are left for another round. `carry`: [`push_client`]'s
/// entries to send unchanged (and then only that one client is pushed).
#[allow(clippy::too_many_arguments)]
fn push_once(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    only: Option<&str>,
    round_clients: usize,
    rep: &mut PushReport,
    done: &mut BTreeSet<String>,
    listed: &mut Option<Mutex<HashSet<String>>>,
    carry: Option<&[crate::layout::DocEntry]>,
) -> Result<bool> {
    let client_only = carry.is_some();
    let head = s.head()?.unwrap_or_default();
    let seen_before = catalog::last_rev(conn, &s.device_id)?;
    let writer = vf_core::new_id();
    let pending = catalog::pending(conn)?;
    let uploaded = catalog::uploaded(conn)?;
    // The admin password file deleted here after it was exchanged (the forgotten-password
    // reset): sent as removed, so the other places drop it too (owner 2026-11-04).
    let pin = vf_fs::trash::pin_file(root);
    if !pin.exists() && catalog::files_base(conn)?.contains_key("trash_pin.json") {
        vf_fs::cabinet::write_replace(&pin, vf_fs::trash::REMOVED_MARKER.as_bytes())
            .map_err(|e| SyncError::Local(e.to_string()))?;
    }
    let files = master_files(root);
    let hashes = files_hashes(&files);
    let files_dirty = hashes != catalog::files_base(conn)?;

    let mut new_head = head.clone();
    // What was sent, as sent (the live rows may change while the push runs): the base later.
    let mut sent_docs: Vec<SentDoc> = vec![];
    let mut sent_segments: Vec<(String, u64, Option<String>, Option<String>)> = vec![];
    let mut segments = 0;

    // A segment this PC has not applied (a pull left documents for later) is never sent: it
    // would put back what the other workstation changed.
    let applied = |c: &str| -> Result<bool> {
        let theirs = if c == CABINET {
            head.cabinet_segment_rev
        } else {
            head.segments.get(c).copied().unwrap_or(0)
        };
        Ok(theirs == catalog::segment_rev(conn, c)? || theirs == 0)
    };
    // This round's clients.
    let waiting: Vec<String> = pending
        .keys()
        .filter(|c| c.as_str() != CABINET && !done.contains(*c))
        .filter(|c| only.is_none_or(|o| o == c.as_str()))
        .cloned()
        .collect();
    let mut round: Vec<String> = vec![];
    let mut docs_in_round = 0;
    for c in &waiting {
        if round.len() >= round_clients || (docs_in_round >= ROUND_DOCS && !round.is_empty()) {
            break;
        }
        docs_in_round += pending.get(c).copied().unwrap_or(0);
        round.push(c.clone());
    }
    let more = waiting.len() > round.len();
    let cabinet_too = !client_only
        && !done.contains(CABINET)
        && (files_dirty || (pending.contains_key(CABINET) && only.is_none_or(|o| o == CABINET)))
        && applied(CABINET)?;

    // Each client's entries, and the contents they need up.
    struct Part {
        client: String,
        folder: String,
        sentinel: String,
        docs: Vec<crate::layout::DocEntry>,
    }
    let mut parts: Vec<Part> = vec![];
    let mut jobs: BTreeMap<String, PathBuf> = BTreeMap::new();
    for client in &round {
        if !applied(client)? {
            rep.skipped += pending.get(client).copied().unwrap_or(0);
            continue;
        }
        let Some(folder) = catalog::client_folder(conn, client)? else {
            continue; // a client with no folder has no segment
        };
        let sentinel_path = filer_intake::sentinel::sentinel_path(&root.join(&folder));
        let Ok(sentinel) = std::fs::read_to_string(&sentinel_path) else {
            continue;
        };
        let docs = catalog::entries(conn, &s.keys, client, &folder)?;
        for e in &docs {
            if e.deleted_at.is_none() && !uploaded.contains(&e.hash) {
                jobs.entry(e.hash.clone())
                    .or_insert_with(|| root.join(&folder).join(&e.rel_path));
            }
            for v in &e.versions {
                if !uploaded.contains(&v.hash) {
                    jobs.entry(v.hash.clone())
                        .or_insert_with(|| vf_fs::cabinet::version_path(root, &v.hash));
                }
            }
        }
        parts.push(Part {
            client: client.clone(),
            folder,
            sentinel,
            docs,
        });
    }
    let cabinet_docs = if cabinet_too {
        let docs = catalog::entries(conn, &s.keys, CABINET, "")?;
        for e in &docs {
            if e.deleted_at.is_none() && !uploaded.contains(&e.hash) {
                jobs.entry(e.hash.clone())
                    .or_insert_with(|| root.join(&e.rel_path));
            }
            for v in &e.versions {
                if !uploaded.contains(&v.hash) {
                    jobs.entry(v.hash.clone())
                        .or_insert_with(|| vf_fs::cabinet::version_path(root, &v.hash));
                }
            }
        }
        Some(docs)
    } else {
        None
    };

    // Contents up, several at once. Many to send: the bucket's objects are listed once
    // rather than asked about one by one.
    if jobs.len() > LIST_ABOVE && listed.is_none() {
        let keys: HashSet<String> = s.remote.list("objects")?.into_iter().collect();
        *listed = Some(Mutex::new(keys));
    }
    let up = upload_all(s, jobs, listed.as_ref())?;
    rep.objects += up.objects;
    rep.bytes += up.bytes;
    {
        let tx = vf_core::db::begin_write(conn)?;
        for (h, ok) in &up.done {
            if *ok {
                tx.execute("INSERT OR IGNORE INTO sync_object (hash) VALUES (?1)", [h])?;
            }
        }
        tx.commit()?;
    }
    let sent = |h: &str| uploaded.contains(h) || up.done.get(h).copied().unwrap_or(false);

    'clients: for mut p in parts {
        for e in &mut p.docs {
            if e.deleted_at.is_none() && !sent(&e.hash) {
                // A file that cannot be read now: the client's segment waits (the bucket keeps
                // its previous one, nothing is dropped from it).
                rep.skipped += pending.get(&p.client).copied().unwrap_or(1);
                continue 'clients;
            }
            e.versions.retain(|v| sent(&v.hash));
        }
        for e in &p.docs {
            sent_docs.push(SentDoc::of(e, Some(&p.client), &p.folder));
        }
        // Entries this catalog does not know go up as the bucket had them (their objects are
        // there already); they are not this catalog's to take as sent.
        for c in carry.unwrap_or_default() {
            if !p.docs.iter().any(|d| d.doc_id == c.doc_id) {
                p.docs.push(c.clone());
            }
        }
        let rev = head.segments.get(&p.client).copied().unwrap_or(0) + 1;
        let seg = ClientSegment {
            client_id: p.client.clone(),
            segment_rev: rev,
            folder_rel: p.folder.clone(),
            sentinel: p.sentinel,
            documents: p.docs,
            empty_dirs: catalog::empty_dirs(root, &p.folder),
            written_by: s.device_name.clone(),
        };
        s.put_json(&layout::segment_key_by(&p.client, rev, &writer), &seg)?;
        new_head.segments.insert(p.client.clone(), rev);
        new_head
            .segment_writers
            .insert(p.client.clone(), writer.clone());
        sent_segments.push((
            p.client.clone(),
            rev,
            Some(p.folder),
            Some(sha256(seg.sentinel.as_bytes())),
        ));
        segments += 1;
    }
    if let Some(mut docs) = cabinet_docs {
        let ok = docs.iter().all(|e| e.deleted_at.is_some() || sent(&e.hash));
        if ok {
            for e in &mut docs {
                e.versions.retain(|v| sent(&v.hash));
            }
            for e in &docs {
                sent_docs.push(SentDoc::of(e, None, ""));
            }
            let rev = head.cabinet_segment_rev + 1;
            let seg = CabinetSegment {
                segment_rev: rev,
                files,
                documents: docs,
                written_by: s.device_name.clone(),
            };
            s.put_json(&layout::cabinet_segment_key_by(rev, &writer), &seg)?;
            new_head.cabinet_segment_rev = rev;
            new_head.cabinet_writer = writer.clone();
            sent_segments.push((CABINET.into(), rev, None, None));
            segments += 1;
        } else {
            rep.skipped += 1;
        }
    }
    push_audit(conn, root, s)?;
    let mark_done = |done: &mut BTreeSet<String>| {
        done.extend(round.iter().cloned());
        if cabinet_too {
            done.insert(CABINET.into());
        }
    };
    if segments == 0 {
        rep.head_rev = head.rev;
        mark_done(done);
        return Ok(more);
    }

    // The commit point. Another push that committed while this one uploaded (its HEAD moved
    // on) wins: this one pulls and starts again, so neither change is lost.
    let now_there = s.head()?.unwrap_or_default();
    if now_there.rev != head.rev || now_there.writer != head.writer {
        return Err(SyncError::Raced);
    }
    new_head.rev = head.rev + 1;
    new_head.written_at = vf_core::now_rfc3339();
    new_head.device_id = s.device_id.clone();
    new_head.device_name = s.device_name.clone();
    new_head.writer = writer;
    s.put_json(layout::HEAD, &new_head)?;
    let back: Head = s.head()?.unwrap_or_default();
    if back.writer != new_head.writer {
        return Err(SyncError::Raced);
    }

    // What was sent is now the base.
    let tx = vf_core::db::begin_write(conn)?;
    for d in &sent_docs {
        catalog::set_base_values(
            &tx,
            &d.doc_id,
            d.client.as_deref(),
            &d.rel_path,
            &d.hash,
            d.rev,
            d.deleted,
        )?;
    }
    for (c, rev, folder, sh) in &sent_segments {
        let fh = (c == CABINET).then(|| serde_json::to_string(&hashes).expect("serializes"));
        catalog::set_segment(
            &tx,
            c,
            *rev,
            folder.as_deref(),
            sh.as_deref(),
            fh.as_deref(),
        )?;
    }
    for d in &sent_docs {
        tx.execute("DELETE FROM sync_pending WHERE doc_id = ?1", [&d.doc_id])?;
    }
    // One client pushed: this computer has still seen only what it had (the other segments of
    // the revs between are not applied here).
    let seen = if client_only {
        seen_before
    } else {
        new_head.rev
    };
    catalog::set_state(&tx, &s.device_id, seen, "push")?;
    tx.commit()?;
    mark_done(done);
    rep.segments += segments;
    rep.documents += sent_docs.len();
    rep.head_rev = new_head.rev;
    if !client_only {
        write_device(s, new_head.rev)?;
    }
    Transaction::new(LABEL)
        .audit(AuditEntry::new(
            LABEL,
            None,
            json!({
                "objects": up.objects, "bytes": up.bytes, "segments": segments,
                "documents": sent_docs.len(), "rev": rep.head_rev, "skipped": rep.skipped,
            }),
        ))
        .commit(conn, &mut NoFileOps, &Actor::device(&s.device_id))?;
    tracing::info!(
        objects = up.objects,
        segments,
        rev = rep.head_rev,
        "sync push"
    );
    Ok(more)
}

/// What [`upload_all`] did.
struct Uploaded {
    /// Content hash → sent or already in the bucket (`true`), or the local file cannot be
    /// read or no longer has this content (`false`: edited outside Vibe Files).
    done: BTreeMap<String, bool>,
    objects: usize,
    bytes: u64,
}

/// Sends the contents `jobs` (hash → file) not in the bucket yet, [`UPLOADERS`] at a time;
/// files of [`BIG`] or more one at a time; at most [`set_upload_cap`]'s rate. A bucket error
/// stops the lot and is returned.
fn upload_all(
    s: &Sync,
    jobs: BTreeMap<String, PathBuf>,
    listed: Option<&Mutex<HashSet<String>>>,
) -> Result<Uploaded> {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    let queue = Mutex::new(jobs.into_iter().collect::<Vec<_>>());
    let done = Mutex::new(BTreeMap::new());
    let failed: Mutex<Option<SyncError>> = Mutex::new(None);
    let stop = AtomicBool::new(false);
    let big = Mutex::new(());
    let (objects, bytes) = (AtomicUsize::new(0), AtomicU64::new(0));
    let start = std::time::Instant::now();
    let one = |hash: &str, path: &Path| -> Result<bool> {
        let key = layout::object_key(&s.keys.object_name(hash));
        let there = match listed {
            Some(l) => l.lock().map(|l| l.contains(&key)).unwrap_or(false),
            None => s.remote.exists(&key)?,
        };
        if there {
            return Ok(true);
        }
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let _one_big = (size >= BIG).then(|| big.lock());
        let Ok(body) = std::fs::read(path) else {
            return Ok(false);
        };
        if sha256(&body) != hash {
            return Ok(false);
        }
        let n = body.len() as u64;
        s.remote.put(&key, s.keys.seal(&body, &key))?;
        drop(body);
        objects.fetch_add(1, Ordering::Relaxed);
        let total = bytes.fetch_add(n, Ordering::Relaxed) + n;
        if let Some(l) = listed
            && let Ok(mut l) = l.lock()
        {
            l.insert(key);
        }
        // The cap: wait until the bytes sent so far are due.
        let cap = UPLOAD_CAP.load(Ordering::Relaxed);
        if cap > 0 {
            let due = std::time::Duration::from_secs_f64(total as f64 / cap as f64);
            if let Some(wait) = due.checked_sub(start.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        Ok(true)
    };
    std::thread::scope(|sc| {
        for _ in 0..UPLOADERS {
            sc.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let Some((hash, path)) = queue.lock().ok().and_then(|mut q| q.pop()) else {
                        break;
                    };
                    match one(&hash, &path) {
                        Ok(ok) => {
                            if let Ok(mut d) = done.lock() {
                                d.insert(hash, ok);
                            }
                        }
                        Err(e) => {
                            stop.store(true, Ordering::Relaxed);
                            if let Ok(mut f) = failed.lock() {
                                f.get_or_insert(e);
                            }
                        }
                    }
                }
            });
        }
    });
    let done = done.into_inner().unwrap_or_default();
    if let Some(e) = failed.into_inner().ok().flatten() {
        return Err(e);
    }
    Ok(Uploaded {
        done,
        objects: objects.into_inner(),
        bytes: bytes.into_inner(),
    })
}

/// A document as it went into a segment.
struct SentDoc {
    doc_id: String,
    client: Option<String>,
    /// Cabinet-relative.
    rel_path: String,
    hash: String,
    rev: i64,
    deleted: bool,
}

impl SentDoc {
    fn of(e: &crate::layout::DocEntry, client: Option<&str>, folder: &str) -> SentDoc {
        SentDoc {
            doc_id: e.doc_id.clone(),
            client: client.map(str::to_owned),
            rel_path: if folder.is_empty() {
                e.rel_path.clone()
            } else {
                format!("{folder}/{}", e.rel_path)
            },
            hash: e.hash.clone(),
            rev: e.rev,
            deleted: e.deleted_at.is_some(),
        }
    }
}

pub(crate) fn write_device(s: &Sync, acked: u64) -> Result<()> {
    s.put_json(
        &layout::device_key(&s.device_id),
        &DeviceRecord {
            device_id: s.device_id.clone(),
            name: s.device_name.clone(),
            mode: "local".into(),
            last_seen: vf_core::now_rfc3339(),
            acked_rev: acked,
        },
    )
}

/// `_Vibe/audit/<month>.jsonl` files changed since last sent go to `audit/<month>`.
fn push_audit(conn: &Connection, root: &Path, s: &Sync) -> Result<()> {
    let dir = root.join(vf_fs::cabinet::VIBE_DIR).join("audit");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let Some(month) = n.strip_suffix(".jsonl") else {
            continue;
        };
        let Ok(bytes) = std::fs::read(e.path()) else {
            continue;
        };
        let h = sha256(&bytes);
        let k = format!("sync_audit:{month}");
        let sent: Option<String> = conn
            .query_row("SELECT v FROM local_state WHERE k = ?1", [&k], |r| r.get(0))
            .ok();
        if sent.as_deref() == Some(h.as_str()) {
            continue;
        }
        let key = layout::audit_key(month);
        s.remote.put(&key, s.keys.seal(&bytes, &key))?;
        conn.execute(
            "INSERT INTO local_state (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            [&k, &h],
        )?;
    }
    Ok(())
}
