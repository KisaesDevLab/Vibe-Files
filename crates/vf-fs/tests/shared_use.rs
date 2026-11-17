//! Several computers on one cabinet at the same time (owner 2026-11-17: "simulate it here
//! first"). Each thread is a computer: its own catalog, its own connection, its own cabinet
//! locks (they are per thread), all working in one cabinet folder, reached through a network
//! path of this PC (`\\localhost\<drive>$\…`) when that share answers, so file opens, renames
//! and locks go through the network redirector as they do on the firm's share. Without the
//! share (no administrative share, not an administrator) the cabinet is used by its local
//! path, which still runs every computer against the others.
//!
//! These are threads of one process. They show that the cabinet's locks, the catalogs and
//! the shared files hold up under several workers over a network path; they do not run
//! several copies of the program, on several machines, each with its own window, worker
//! threads and Windows login. `docs/MULTI_USER_CHECK.md` is for that.
//!
//! What must hold however the work interleaves:
//! - a shared list several computers add to at once holds every addition (no save writes
//!   over another's);
//! - computers that index the cabinet at once end with the same document ids, and each
//!   document has one record in its folder, not one per computer;
//! - nobody is told a wrong cause: a catalog or file that is only busy is never "not a Vibe
//!   Files catalog", "not in the catalog" or "is not a Vibe Files cabinet".

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use filer_core::clients::Client;
use uuid::Uuid;
use vf_core::db::{Connection, Storage};
use vf_fs::cabinet::{self, CabinetFile};

/// Messages that name a wrong cause for something that is only busy or momentarily away.
const WRONG_CAUSES: &[&str] = &[
    "not a Vibe Files catalog",
    "is not a Vibe Files cabinet",
    "is not in the catalog",
    "went ahead meanwhile",
];

/// `dir` as other computers would reach it, when this PC's administrative share answers.
fn over_the_network(dir: &Path) -> PathBuf {
    let s = dir.to_string_lossy();
    let unc = s
        .strip_prefix(r"\\?\")
        .unwrap_or(&s)
        .split_once(":\\")
        .map(|(drive, rest)| PathBuf::from(format!(r"\\localhost\{drive}$\{rest}")));
    match unc {
        Some(p) if p.is_dir() => p,
        _ => dir.to_path_buf(),
    }
}

struct Cab {
    _tmp: tempfile::TempDir,
    /// The cabinet as every computer opens it.
    root: PathBuf,
    /// Where each computer keeps its own catalog (local, as on a real PC).
    data: PathBuf,
    clients: Vec<String>,
}

/// A cabinet of `clients` invented clients, each with a folder, a client file and `docs`
/// documents.
fn cab(clients: usize, docs: usize) -> Cab {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("cabinet");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let firm = Uuid::new_v4();
    cabinet::create(
        &local,
        &CabinetFile {
            cabinet_id: vf_core::new_id(),
            firm_id: firm.to_string(),
            schema_version: vf_core::db::SCHEMA_VERSION,
            layout_version: 1,
            created: vf_core::now_rfc3339(),
            name: None,
        },
    )
    .unwrap();
    let mut ids = Vec::new();
    for c in 0..clients {
        let id = format!("{}", 1000 + c);
        let folder = local.join(format!("Invented Client {c} ({id})"));
        std::fs::create_dir_all(folder.join("Income Tax/2025")).unwrap();
        filer_intake::sentinel::write_sentinel(
            &local,
            &folder,
            &Client {
                id: id.clone(),
                uuid: Uuid::new_v4(),
                name: format!("Invented Client {c}"),
                extra: Default::default(),
            },
            firm,
        )
        .unwrap();
        for d in 0..docs {
            std::fs::write(
                folder.join(format!("Income Tax/2025/doc {d}.pdf")),
                format!("client {c} document {d}"),
            )
            .unwrap();
        }
        ids.push(id);
    }
    vf_fs::doctypes::seed_if_missing(&local).unwrap();
    let root = over_the_network(&local);
    // Said, so a run shows which it was (`cargo test -- --nocapture`).
    println!(
        "cabinet {} ({})",
        root.display(),
        if root == local {
            "by its local path: this PC's administrative share did not answer"
        } else {
            "over the network path"
        }
    );
    Cab {
        _tmp: tmp,
        root,
        data,
        clients: ids,
    }
}

fn catalog(c: &Cab, pc: usize) -> Connection {
    vf_core::db::open(&c.data.join(format!("pc-{pc}.db")), Storage::Local).unwrap()
}

/// `work` again while the reason it failed is another computer being at the same thing
/// (the cabinet lock held, the catalog busy): what a person does when told "try again in a
/// moment". Every message seen is kept, to look for wrong causes afterwards.
fn until_done<T>(said: &mut Vec<String>, mut work: impl FnMut() -> Result<T, String>) -> T {
    let start = std::time::Instant::now();
    loop {
        match work() {
            Ok(t) => return t,
            Err(e) => {
                assert!(
                    start.elapsed() < std::time::Duration::from_secs(300),
                    "never done: {e}"
                );
                said.push(e);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
}

fn no_wrong_cause(said: &[String]) {
    for m in said {
        for w in WRONG_CAUSES {
            assert!(!m.contains(w), "a wrong cause was named: {m}");
        }
    }
}

/// Six computers each add 20 entries to two shared lists at once (the payer names, under
/// their own lock; the "not the same client" marks, under the client list's lock). Every
/// entry of every computer is there at the end.
#[test]
fn several_computers_adding_to_a_shared_list_lose_nothing() {
    const PCS: usize = 6;
    const EACH: usize = 20;
    let c = Arc::new(cab(2, 1));
    let start = Arc::new(Barrier::new(PCS));
    let threads: Vec<_> = (0..PCS)
        .map(|pc| {
            let (c, start) = (c.clone(), start.clone());
            std::thread::spawn(move || {
                let mut said = Vec::new();
                start.wait();
                for i in 0..EACH {
                    until_done(&mut said, || {
                        vf_fs::aliases::learn(&c.root, &format!("Payer {pc}-{i} Inc"), "1000")
                    });
                    until_done(&mut said, || {
                        vf_fs::similar::dismiss(
                            &c.root,
                            &format!("pc{pc}-a{i}"),
                            &format!("pc{pc}-b{i}"),
                        )
                    });
                }
                said
            })
        })
        .collect();
    let said: Vec<String> = threads
        .into_iter()
        .flat_map(|t| t.join().unwrap())
        .collect();
    no_wrong_cause(&said);
    let payers: BTreeSet<String> = vf_fs::aliases::load(&c.root)
        .into_iter()
        .map(|a| a.payer)
        .collect();
    let marks = vf_fs::similar::dismissed(&c.root).unwrap();
    for pc in 0..PCS {
        for i in 0..EACH {
            assert!(
                payers.contains(&format!("Payer {pc}-{i} Inc")),
                "a payer name of computer {pc} was written over (entry {i}); {} of {} kept",
                payers.len(),
                PCS * EACH
            );
            assert!(
                marks.contains(&(format!("pc{pc}-a{i}"), format!("pc{pc}-b{i}"))),
                "a mark of computer {pc} was written over (entry {i}); {} of {} kept",
                marks.len(),
                PCS * EACH
            );
        }
    }
    assert_eq!((payers.len(), marks.len()), (PCS * EACH, PCS * EACH));
}

/// Live documents of a catalog: path → id.
fn documents(conn: &Connection) -> BTreeMap<String, String> {
    conn.prepare("SELECT rel_path, id FROM document WHERE deleted_at IS NULL")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Four computers index a cabinet new to all of them at the same moment (the morning every
/// session host starts). Each ends with every document, under the same id on every
/// computer, and each client folder's records name each document once.
///
/// What this shows: `rebuild::index` takes the cabinet's index lock, so the four take their
/// turn; the test proves that the turn-taking holds through the network path and that the
/// later ones take the first one's ids from the folders' records. It does not show four
/// indexes writing records at the same instant: the lock is what keeps that from happening.
#[test]
fn computers_indexing_at_once_agree_on_every_document() {
    const PCS: usize = 4;
    let c = Arc::new(cab(12, 6));
    let start = Arc::new(Barrier::new(PCS));
    let threads: Vec<_> = (0..PCS)
        .map(|pc| {
            let (c, start) = (c.clone(), start.clone());
            std::thread::spawn(move || {
                let mut conn = catalog(&c, pc);
                let mut said = Vec::new();
                start.wait();
                until_done(&mut said, || {
                    vf_fs::rebuild::index(&mut conn, &c.root, &format!("pc-{pc}"))
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
                // And once more, as at the next open: nothing new, nothing changed.
                until_done(&mut said, || {
                    vf_fs::rebuild::index(&mut conn, &c.root, &format!("pc-{pc}"))
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
                (documents(&conn), said)
            })
        })
        .collect();
    let done: Vec<(BTreeMap<String, String>, Vec<String>)> =
        threads.into_iter().map(|t| t.join().unwrap()).collect();
    for (_, said) in &done {
        no_wrong_cause(said);
    }
    let first = &done[0].0;
    assert_eq!(first.len(), 12 * 6, "every document is cataloged");
    for (pc, (docs, _)) in done.iter().enumerate() {
        assert_eq!(
            docs.len(),
            first.len(),
            "computer {pc} has another number of documents"
        );
        let differ: Vec<&String> = docs
            .iter()
            .filter(|(path, id)| first.get(*path) != Some(*id))
            .map(|(path, _)| path)
            .collect();
        assert!(
            differ.is_empty(),
            "computer {pc} knows {} documents under another id than computer 0 (e.g. {})",
            differ.len(),
            differ[0]
        );
    }
    // One record a document in each folder's docmeta.jsonl, with the id every computer has.
    let ids: BTreeSet<&String> = first.values().collect();
    for id in &c.clients {
        let folder = std::fs::read_dir(&c.root)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.to_string_lossy().ends_with(&format!("({id})")))
            .unwrap();
        let text = std::fs::read_to_string(folder.join("_Vibe/docmeta.jsonl")).unwrap();
        let recorded: Vec<String> = text
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string))
            .collect();
        let distinct: BTreeSet<&String> = recorded.iter().collect();
        assert_eq!(
            distinct.len(),
            6,
            "client {id}: its records name {} ids for 6 documents",
            distinct.len()
        );
        assert!(
            distinct.iter().all(|i| ids.contains(*i)),
            "client {id}: a record names an id no catalog has"
        );
    }
}

/// Two sessions on one computer share its catalog (two logins on a session host): one
/// indexes while the other keeps opening the catalog and reading it, as the window, the
/// audit check and the startup check do. The reader is never told the catalog is something
/// it is not, and reads a whole catalog once the index is done.
#[test]
fn a_session_reads_the_catalog_another_session_is_indexing() {
    let c = Arc::new(cab(10, 8));
    let path = c.data.join("pc-0.db");
    drop(catalog(&c, 0));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let (path, done) = (path.clone(), done.clone());
        std::thread::spawn(move || {
            let (mut said, mut reads) = (Vec::new(), 0usize);
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                match vf_core::db::open(&path, Storage::Local).map_err(|e| e.to_string()) {
                    Ok(conn) => {
                        match vf_core::audit::verify(&conn) {
                            Ok(r) => assert!(r.broken.is_none(), "the audit chain reads broken"),
                            Err(e) => said.push(e.to_string()),
                        }
                        if let Err(e) = conn.query_row(
                            "SELECT count(*) FROM document WHERE deleted_at IS NULL",
                            [],
                            |r| r.get::<_, i64>(0),
                        ) {
                            said.push(vf_core::db::missing_or(e, "is not in the catalog"));
                        }
                        reads += 1;
                    }
                    Err(e) => said.push(e),
                }
            }
            (said, reads)
        })
    };
    let mut conn = catalog(&c, 0);
    vf_fs::rebuild::index(&mut conn, &c.root, "pc-0").unwrap();
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    let (said, reads) = reader.join().unwrap();
    no_wrong_cause(&said);
    assert!(reads > 0, "the other session never got to read: {said:?}");
    assert_eq!(documents(&conn).len(), 10 * 8);
}
