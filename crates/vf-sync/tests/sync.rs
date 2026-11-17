//! PHASES P11 done-whens 1–4 against an in-memory bucket (the S3 path is the same calls; an
//! env-gated test below runs them against a real endpoint). Two "PCs" = two cabinet folders and
//! two catalogs; a third, empty folder is the bare PC. All names are invented.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use object_store::ObjectStore;
use vf_core::db::{Connection, Storage};
use vf_fs::cabinet::CabinetFile;
use vf_security::keys::KdfParams;
use vf_sync::{Keys, Remote, Sync};

const PASS: &str = "correct horse battery staple";
const FAST: KdfParams = KdfParams {
    // 1 MiB and 2 passes: the least the portal's Argon2 takes (it opens the bucket
    // `writes_a_bucket_for_the_portals_tests` writes).
    m_kib: 1024,
    t: 2,
    p: 1,
};

struct Pc {
    root: PathBuf,
    conn: Connection,
    sync: Sync,
}

fn catalog(dir: &Path) -> Connection {
    std::fs::create_dir_all(dir).unwrap();
    vf_core::db::open(&dir.join("catalog.db"), Storage::Local).unwrap()
}

/// PC-A: a cabinet with two clients and a few documents, sync set up.
fn pc_a(t: &Path, store: Arc<dyn ObjectStore>) -> Pc {
    let root = t.join("a").join("Clients");
    let cabinet_id = vf_core::new_id();
    vf_fs::cabinet::create(
        &root,
        &CabinetFile {
            cabinet_id: cabinet_id.clone(),
            firm_id: uuid::Uuid::new_v4().to_string(),
            schema_version: vf_core::db::SCHEMA_VERSION,
            layout_version: 1,
            created: vf_core::now_rfc3339(),
            name: Some("Firm".into()),
        },
    )
    .unwrap();
    let mut conn = catalog(&t.join("a").join("data"));
    vf_fs::doctypes::seed_if_missing(&root).unwrap();
    vf_fs::retention::seed_if_missing(&root).unwrap();
    vf_fs::retention::sync(&mut conn, &root, "dev-a", "seed").unwrap();
    vf_fs::doctypes::sync(&mut conn, &root, "dev-a", "seed").unwrap();
    for (id, name) in [("1042", "Anderson Farms LLC"), ("2002", "Bell Ranch Inc")] {
        let p = vf_fs::newclient::plan(
            &conn,
            &root,
            &vf_fs::newclient::NewClient {
                ext_id: id.into(),
                name: name.into(),
                entity_type: None,
                partner: String::new(),
            },
        )
        .unwrap();
        vf_fs::newclient::create(&mut conn, &root, &p, "dev-a").unwrap();
    }
    put(
        &root,
        "Anderson Farms LLC (1042)/Income Tax/2025/W-2 Northwind.pdf",
        "w2 2025 invented",
    );
    put(
        &root,
        "Anderson Farms LLC (1042)/Permanent/Setup/EIN letter.pdf",
        "ein invented",
    );
    put(
        &root,
        "Bell Ranch Inc (2002)/Payroll/2025/941 Q1.pdf",
        "941 invented",
    );
    vf_fs::rebuild::rebuild(&mut conn, &root, "dev-a").unwrap();
    set_type(&mut conn, &root, "W-2 Northwind.pdf", "W-2", Some(2025));
    // Every year goes to B2 here (PA-53's rule has its own test): these tests do not depend
    // on today's date.
    vf_fs::clients::set_b2_years_back(&root, Some(0)).unwrap();
    let remote = Remote::new(store, &cabinet_id);
    let keys = vf_sync::setup::init(&root, &remote, PASS, FAST).unwrap();
    Pc {
        root,
        conn,
        sync: session(remote, keys, &cabinet_id, "dev-a", "OFFICE-PC"),
    }
}

fn session(remote: Remote, keys: Keys, cabinet_id: &str, dev: &str, name: &str) -> Sync {
    Sync {
        remote,
        keys,
        cabinet_id: cabinet_id.into(),
        device_id: dev.into(),
        device_name: name.into(),
    }
}

fn put(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn doc_id(conn: &Connection, file: &str) -> String {
    conn.query_row(
        "SELECT id FROM document WHERE rel_path LIKE '%/' || ?1 AND deleted_at IS NULL",
        [file],
        |r| r.get(0),
    )
    .unwrap()
}

fn set_type(conn: &mut Connection, root: &Path, file: &str, ty: &str, year: Option<i32>) {
    let t: String = conn
        .query_row("SELECT id FROM doc_type WHERE name = ?1", [ty], |r| {
            r.get(0)
        })
        .unwrap();
    let id = doc_id(conn, file);
    vf_fs::docmeta::edit(
        conn,
        root,
        &[id],
        &vf_fs::docmeta::MetaEdit {
            doc_type: Some(Some(t)),
            tax_year: Some(year),
            ..Default::default()
        },
        "dev",
    )
    .unwrap();
}

/// Every file outside `_Vibe` → SHA-256, and the versions store.
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for f in vf_fs::rebuild::cabinet_files(root) {
        out.insert(
            vf_fs::rebuild::rel(root, &f),
            filer_intake::commit::sha256_file(&f).unwrap(),
        );
    }
    let v = root.join("_Vibe/versions");
    for f in vf_fs::rebuild::cabinet_files(&v) {
        if f.extension().is_none() {
            out.insert(
                format!("versions/{}", vf_fs::rebuild::rel(&v, &f)),
                filer_intake::commit::sha256_file(&f).unwrap(),
            );
        }
    }
    out
}

fn meta(conn: &Connection, id: &str) -> (String, String, Option<String>, Option<i64>, String) {
    conn.query_row(
        "SELECT d.rel_path, d.hash, t.name, d.tax_year, d.status FROM document d
         LEFT JOIN doc_type t ON t.id = d.doc_type_id WHERE d.id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )
    .unwrap()
}

/// PC-B: the bare-PC restore of what PC-A pushed.
fn pc_b(t: &Path, store: Arc<dyn ObjectStore>, a: &Pc) -> Pc {
    let root = t.join("b").join("Clients");
    let mut conn = catalog(&t.join("b").join("data"));
    vf_sync::restore::restore(
        &mut conn,
        &root,
        store.clone(),
        &a.sync.cabinet_id,
        PASS,
        "dev-b",
        "LAPTOP",
    )
    .unwrap();
    let sync = Sync::unlock(&root, store, PASS, "dev-b", "LAPTOP").unwrap();
    Pc { root, conn, sync }
}

#[test]
fn round_trip_restore_and_bound_folders() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    let r = vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert_eq!(r.head_rev, 1);
    assert!(r.objects >= 3);
    assert!(
        vf_sync::catalog::pending(&a.conn, &vf_sync::rules::Rules::load(&a.root).unwrap())
            .unwrap()
            .is_empty()
    );
    // Nothing readable in the bucket's names or bodies.
    let keys = Remote::new(store.clone(), &a.sync.cabinet_id)
        .list("")
        .unwrap();
    assert!(
        keys.iter()
            .all(|k| !k.contains("Anderson") && !k.contains("1042"))
    );

    // Done-when 4: a bare PC + passphrase + bucket = the same files, byte for byte.
    let wrong = vf_sync::restore::restore(
        &mut catalog(&t.path().join("x")),
        &t.path().join("x").join("Clients"),
        store.clone(),
        &a.sync.cabinet_id,
        "wrong horse battery staple",
        "dev-x",
        "X",
    );
    assert!(matches!(wrong, Err(vf_sync::SyncError::WrongPassphrase)));
    let mut b = pc_b(t.path(), store.clone(), &a);
    assert_eq!(snapshot(&b.root), snapshot(&a.root));

    // Done-when 2: same client_id, same folder name, Bound in the Filer root scan.
    let list = vf_fs::clients::load(&b.root).unwrap().list.unwrap();
    let scan = filer_intake::sentinel::scan_root(
        &filer_intake::sentinel::ScanInput {
            root: b.root.clone(),
            clients: Arc::new(list),
            id_pattern: "[A-Z0-9]{4,}".into(),
            threshold: 0.8,
            ignored: vec![],
        },
        &Default::default(),
    )
    .unwrap();
    for f in ["Anderson Farms LLC (1042)", "Bell Ranch Inc (2002)"] {
        let bnd = scan.iter().find(|x| x.name() == f).unwrap();
        assert_eq!(bnd.status, filer_intake::sentinel::Status::Bound, "{f}");
        let sa = std::fs::read(a.root.join(f).join("_Vibe/client.json")).unwrap();
        let sb = std::fs::read(b.root.join(f).join("_Vibe/client.json")).unwrap();
        assert_eq!(sa, sb, "the sentinel comes back byte for byte");
    }
    let w2 = doc_id(&a.conn, "W-2 Northwind.pdf");
    assert_eq!(meta(&b.conn, &w2), meta(&a.conn, &w2));

    // Done-when 1: a file in on PC-A appears on PC-B with the same hash and metadata.
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Income Tax/2025/1099-INT Lakeside.pdf",
        "1099 invented",
    );
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    set_type(
        &mut a.conn,
        &a.root,
        "1099-INT Lakeside.pdf",
        "1099-INT",
        Some(2025),
    );
    assert_eq!(
        vf_sync::catalog::pending(&a.conn, &vf_sync::rules::Rules::load(&a.root).unwrap())
            .unwrap()
            .len(),
        1
    );
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let p = vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync).unwrap();
    assert_eq!(p.new, 1);
    let id = doc_id(&a.conn, "1099-INT Lakeside.pdf");
    assert_eq!(meta(&b.conn, &id), meta(&a.conn, &id));
    assert_eq!(meta(&b.conn, &id).2.as_deref(), Some("1099-INT"));
    // B pulled, so it has nothing to push; and a second pull changes nothing.
    assert!(
        vf_sync::catalog::pending(&b.conn, &vf_sync::rules::Rules::load(&b.root).unwrap())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync)
            .unwrap()
            .changed(),
        0
    );
    // Metadata edited on B goes back to A; a move too.
    set_type(
        &mut b.conn,
        &b.root,
        "1099-INT Lakeside.pdf",
        "1099-INT",
        Some(2024),
    );
    vf_sync::push::push(&mut b.conn, &b.root, &b.sync, None).unwrap();
    let p = vf_sync::pull::pull(&mut a.conn, &a.root, &a.sync).unwrap();
    assert_eq!(p.meta, 1);
    assert_eq!(meta(&a.conn, &id).3, Some(2024));
    assert_eq!(snapshot(&b.root), snapshot(&a.root));
}

/// A client's notes file (FileCenter's `!fcinfo.ini`, PA-76) is not a document. A row an
/// earlier build made for one is never sent: not as a document, and above all not as a
/// deletion, which would move the file to the trash on every other PC.
#[test]
fn a_notes_file_is_never_sent_or_deleted_by_sync() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    let rel = "Anderson Farms LLC (1042)/!fcinfo.ini";
    put(&a.root, rel, "[Memo]\r\ninvented note\r\n");
    // As an earlier build left it: a live row for the file.
    let like = doc_id(&a.conn, "EIN letter.pdf");
    let hash = filer_intake::commit::sha256_file(&a.root.join(rel)).unwrap();
    a.conn
        .execute(
            "INSERT INTO document (id, client_id, rel_path, hash, size, mime, status, filed_by,
                                   filed_at, device_id, rev, updated_at)
             SELECT 'note-row', client_id, ?2, ?3, 24, mime, status, filed_by, filed_at,
                    device_id, 1, updated_at FROM document WHERE id = ?1",
            [&like, rel, &hash],
        )
        .unwrap();
    let rules = vf_sync::rules::Rules::load(&a.root).unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert!(
        vf_sync::catalog::pending(&a.conn, &rules)
            .unwrap()
            .is_empty()
    );
    let mut b = pc_b(t.path(), store.clone(), &a);
    assert!(!b.root.join(rel).exists(), "not sent as a document");
    let n: i64 = b
        .conn
        .query_row(
            "SELECT count(*) FROM document WHERE id = 'note-row'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0);

    // PC-B has its own notes for the client. On PC-A the row is marked deleted (an index that
    // no longer lists the file does that), and something else changes so the client is sent.
    put(&b.root, rel, "[Memo]\r\nnotes kept on b\r\n");
    a.conn
        .execute(
            "INSERT INTO sync_base (doc_id, client_id, rel_path, hash, rev, deleted)
             SELECT id, client_id, rel_path, hash, rev, 0 FROM document WHERE id = 'note-row'",
            [],
        )
        .unwrap();
    a.conn
        .execute(
            "UPDATE document SET deleted_at = updated_at, tombstone_reason = 'missing on rebuild',
                                 rev = rev + 1 WHERE id = 'note-row'",
            [],
        )
        .unwrap();
    assert!(
        vf_sync::catalog::pending(&a.conn, &rules)
            .unwrap()
            .is_empty(),
        "the deletion is not something to send"
    );
    set_type(&mut a.conn, &a.root, "EIN letter.pdf", "W-2", Some(2024));
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync).unwrap();
    assert_eq!(
        std::fs::read_to_string(b.root.join(rel)).unwrap(),
        "[Memo]\r\nnotes kept on b\r\n"
    );
}

/// PA-79: the clients' notes files are backed up to B2 beside the manifest (they are not
/// documents) and come back with a restore, byte for byte, sub-folders included. Nothing is
/// sent again until notes change.
#[test]
fn notes_files_are_backed_up_and_restored() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    let top = "Anderson Farms LLC (1042)/!fcinfo.ini";
    let sub = "Anderson Farms LLC (1042)/Income Tax/2025/!fcinfo.ini";
    // ANSI, as FileCenter writes: a byte that is not UTF-8.
    let top_bytes = b"[Fields]\r\n\r\n[Memo]\r\ninvented caf\xE9 note\r\n\r\n[System]\r\n".to_vec();
    std::fs::write(a.root.join(top), &top_bytes).unwrap();
    put(&a.root, sub, "[Memo]\r\nyear folder note\r\n");
    // A listing of the client's folder (an index, the weekly sweep) sees the notes files.
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    assert_eq!(vf_fs::fcinfo::waiting(&a.conn).unwrap().len(), 1);
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert!(vf_fs::fcinfo::waiting(&a.conn).unwrap().is_empty());
    // One object for the client, its name and body saying nothing readable.
    let remote = Remote::new(store.clone(), &a.sync.cabinet_id);
    let keys = remote.list("notes/").unwrap();
    assert_eq!(keys.len(), 1);
    assert!(!keys[0].contains("Anderson") && !keys[0].contains("fcinfo"));
    let body = remote.get(&keys[0]).unwrap().unwrap();
    assert!(!body.windows(8).any(|w| w == b"invented"));

    let b = pc_b(t.path(), store.clone(), &a);
    assert_eq!(std::fs::read(b.root.join(top)).unwrap(), top_bytes);
    assert_eq!(
        std::fs::read_to_string(b.root.join(sub)).unwrap(),
        "[Memo]\r\nyear folder note\r\n"
    );
    let rows: i64 = b
        .conn
        .query_row(
            "SELECT count(*) FROM document WHERE lower(rel_path) LIKE '%!fcinfo.ini'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0, "back as files, not as documents");

    // Nothing changed: nothing is sent. Notes saved in Vibe Files: sent with the next push.
    assert_eq!(
        vf_sync::push::push_notes(&a.conn, &a.root, &a.sync).unwrap(),
        0
    );
    let client: String = a
        .conn
        .query_row(
            "SELECT id FROM client WHERE folder_rel = 'Anderson Farms LLC (1042)'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    std::fs::write(a.root.join(top), b"[Memo]\r\nchanged\r\n").unwrap();
    vf_fs::fcinfo::mark_changed(&a.conn, &client).unwrap();
    assert_eq!(
        vf_sync::push::push_notes(&a.conn, &a.root, &a.sync).unwrap(),
        1
    );
    // Notes that are there are never written over by the backup.
    let b_before = std::fs::read(b.root.join(top)).unwrap();
    assert_eq!(
        vf_sync::restore::restore_notes(&b.conn, &b.root, &b.sync).unwrap(),
        0
    );
    assert_eq!(std::fs::read(b.root.join(top)).unwrap(), b_before);
    // A missing one comes back as the backup now has it.
    std::fs::remove_file(b.root.join(top)).unwrap();
    assert_eq!(
        vf_sync::restore::restore_notes(&b.conn, &b.root, &b.sync).unwrap(),
        1
    );
    assert_eq!(
        std::fs::read(b.root.join(top)).unwrap(),
        b"[Memo]\r\nchanged\r\n"
    );
}

#[test]
fn a_forced_collision_keeps_both_and_raises_a_conflict() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let mut b = pc_b(t.path(), store.clone(), &a);
    let id = doc_id(&a.conn, "W-2 Northwind.pdf");

    // Both change the same document while neither has the other's change.
    let edit = |pc: &mut Pc, body: &str| {
        let tmp = vf_fs::content::tmp_file(&pc.root, "pdf").unwrap();
        std::fs::write(&tmp, body).unwrap();
        vf_fs::content::replace(
            &mut pc.conn,
            &pc.root,
            &pc.sync.device_id,
            &id,
            &tmp,
            vf_fs::content::Change {
                label: "annotate",
                created_by: "test",
                reason: "before annotations",
                detail: serde_json::json!({}),
                extra: vec![],
                based_on: None,
            },
        )
        .unwrap()
        .unwrap();
    };
    edit(&mut a, "w2 annotated on the office PC");
    edit(&mut b, "w2 annotated on the laptop");
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    // B's push pulls first and meets A's change.
    let r = vf_sync::push::push(&mut b.conn, &b.root, &b.sync, None).unwrap();
    assert_eq!(r.pulled.as_ref().unwrap().conflicts, 1);
    let rel = "Anderson Farms LLC (1042)/Income Tax/2025/W-2 Northwind.pdf";
    let copy = "Anderson Farms LLC (1042)/Income Tax/2025/W-2 Northwind (conflict from LAPTOP).pdf";
    assert_eq!(
        std::fs::read_to_string(b.root.join(rel)).unwrap(),
        "w2 annotated on the office PC"
    );
    assert_eq!(
        std::fs::read_to_string(b.root.join(copy)).unwrap(),
        "w2 annotated on the laptop",
        "nothing overwritten: the laptop's version is kept beside it"
    );
    let items = vf_sync::restore::conflicts(&b.conn).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].copy_rel_path.as_deref(), Some(copy));
    // The copy went up with B's push; A gets it.
    vf_sync::pull::pull(&mut a.conn, &a.root, &a.sync).unwrap();
    assert_eq!(
        std::fs::read_to_string(a.root.join(copy)).unwrap(),
        "w2 annotated on the laptop"
    );
    // Keep mine on B: the laptop's content becomes the document's; the copy goes to the trash.
    vf_sync::restore::resolve(
        &mut b.conn,
        &b.root,
        "dev-b",
        &items[0].id,
        vf_sync::restore::Keep::Mine,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(b.root.join(rel)).unwrap(),
        "w2 annotated on the laptop"
    );
    assert!(!b.root.join(copy).exists());
    assert!(vf_sync::restore::conflicts(&b.conn).unwrap().is_empty());
    vf_sync::push::push(&mut b.conn, &b.root, &b.sync, None).unwrap();
    vf_sync::pull::pull(&mut a.conn, &a.root, &a.sync).unwrap();
    assert_eq!(
        std::fs::read_to_string(a.root.join(rel)).unwrap(),
        "w2 annotated on the laptop"
    );
    assert!(!a.root.join(copy).exists(), "the trashed copy follows");
    assert_eq!(snapshot(&b.root), snapshot(&a.root));
}

#[test]
fn pushing_one_client_sends_only_its_documents() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    for n in 0..4 {
        put(
            &a.root,
            &format!("Anderson Farms LLC (1042)/Income Tax/2025/doc {n}.pdf"),
            &format!("a {n}"),
        );
    }
    put(
        &a.root,
        "Bell Ranch Inc (2002)/Payroll/2025/941 Q2.pdf",
        "b q2",
    );
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    let anderson: String = a
        .conn
        .query_row("SELECT id FROM client WHERE ext_id = '1042'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let bell: String = a
        .conn
        .query_row("SELECT id FROM client WHERE ext_id = '2002'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let p =
        vf_sync::catalog::pending(&a.conn, &vf_sync::rules::Rules::load(&a.root).unwrap()).unwrap();
    assert_eq!(p.get(&anderson), Some(&4));
    assert_eq!(p.get(&bell), Some(&1));
    let r = vf_sync::push::push(&mut a.conn, &a.root, &a.sync, Some(&anderson)).unwrap();
    assert_eq!(r.objects, 4);
    let p =
        vf_sync::catalog::pending(&a.conn, &vf_sync::rules::Rules::load(&a.root).unwrap()).unwrap();
    assert_eq!(p.get(&anderson), None);
    assert_eq!(p.get(&bell), Some(&1));
}

#[test]
fn deletes_follow_and_disposed_objects_leave_the_bucket() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let mut b = pc_b(t.path(), store.clone(), &a);
    let id = doc_id(&a.conn, "941 Q1.pdf");
    vf_fs::trash::delete(
        &mut a.conn,
        &a.root,
        "dev-a",
        std::slice::from_ref(&id),
        "test",
    )
    .unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let p = vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync).unwrap();
    assert_eq!(p.deleted, 1);
    assert!(
        !b.root
            .join("Bell Ranch Inc (2002)/Payroll/2025/941 Q1.pdf")
            .exists()
    );
    // A trashed document's object stays (the trash can restore it)…
    assert_eq!(vf_sync::restore::remove_disposed(&a.sync).unwrap(), 0);
    // …a disposed one goes, once every workstation has seen the tombstone.
    a.conn
        .execute(
            "UPDATE document SET tombstone_reason = 'disposed', rev = rev + 1 WHERE id = ?1",
            [&id],
        )
        .unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert_eq!(
        vf_sync::restore::remove_disposed(&a.sync).unwrap(),
        0,
        "B has not seen it"
    );
    vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync).unwrap();
    assert_eq!(vf_sync::restore::remove_disposed(&a.sync).unwrap(), 1);
}

/// The same round trip against a real S3 endpoint (B2 or MinIO) when `VF_S3_ENDPOINT`,
/// `VF_S3_REGION`, `VF_S3_BUCKET`, `VF_S3_KEY_ID` and `VF_S3_APP_KEY` are set. Never by default
/// (no network in tests).
#[test]
fn round_trip_on_a_real_endpoint_when_configured() {
    let Ok(endpoint) = std::env::var("VF_S3_ENDPOINT") else {
        return;
    };
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let store = Remote::s3(
        &vf_sync::remote::S3Config {
            endpoint,
            region: env("VF_S3_REGION"),
            bucket: env("VF_S3_BUCKET"),
            key_id: env("VF_S3_KEY_ID"),
        },
        &env("VF_S3_APP_KEY"),
    )
    .unwrap();
    let t = tempfile::tempdir().unwrap();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let b = pc_b(t.path(), store, &a);
    assert_eq!(snapshot(&b.root), snapshot(&a.root));
}

/// Q-38: the key remembered for this Windows login unlocks without the passphrase, and is the
/// same key; forgetting removes it.
#[test]
fn remembered_key_unlocks_without_the_passphrase() {
    let t = tempfile::tempdir().unwrap();
    let a = pc_a(t.path(), Remote::memory());
    let data = t.path().join("data");
    let firm = vf_sync::setup::firm_key_local(&a.root, &a.sync.cabinet_id, PASS).unwrap();
    assert!(vf_sync::setup::recall(&data, &a.sync.cabinet_id).is_none());
    vf_sync::setup::remember(&data, &a.sync.cabinet_id, &firm).unwrap();
    let k = vf_sync::setup::recall(&data, &a.sync.cabinet_id).unwrap();
    let h = vf_sync::crypto::sha256(b"x");
    assert_eq!(k.object_name(&h), a.sync.keys.object_name(&h));
    // Bound to its cabinet: another cabinet id does not open it.
    std::fs::copy(
        vf_sync::setup::remembered_file(&data, &a.sync.cabinet_id),
        vf_sync::setup::remembered_file(&data, "other"),
    )
    .unwrap();
    assert!(vf_sync::setup::recall(&data, "other").is_none());
    vf_sync::setup::forget(&data, &a.sync.cabinet_id);
    assert!(vf_sync::setup::recall(&data, &a.sync.cabinet_id).is_none());
}

/// QA S3 / X4: a manifest naming a path outside the cabinet is refused before anything is
/// written; a HEAD older than one already seen is refused.
#[test]
fn hostile_manifests_are_refused() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let mut b = pc_b(t.path(), store.clone(), &a);
    let head = a.sync.head().unwrap().unwrap();
    let evil = t.path().join("evil.txt");
    let mut seg = vf_sync::layout::CabinetSegment {
        segment_rev: head.cabinet_segment_rev + 1,
        ..Default::default()
    };
    seg.files.insert("../../../evil.txt".into(), "x".into());
    a.sync
        .remote
        .put(
            &vf_sync::layout::cabinet_segment_key(seg.segment_rev),
            a.sync.keys.seal(
                &serde_json::to_vec(&seg).unwrap(),
                &vf_sync::layout::cabinet_segment_key(seg.segment_rev),
            ),
        )
        .unwrap();
    let mut h2 = head.clone();
    h2.rev += 1;
    h2.cabinet_segment_rev = seg.segment_rev;
    let k = vf_sync::layout::HEAD;
    a.sync
        .remote
        .put(k, a.sync.keys.seal(&serde_json::to_vec(&h2).unwrap(), k))
        .unwrap();
    let r = vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync);
    assert!(matches!(r, Err(vf_sync::SyncError::Format(_))), "{r:?}");
    assert!(!evil.exists());
    assert!(!vf_sync::layout::safe_rel("C:/x") && !vf_sync::layout::safe_rel("a/../b"));
    assert!(vf_sync::layout::safe_rel("Income Tax/2025/W-2.pdf"));

    // Put the first HEAD back (as deleting the newest B2 file version would): refused.
    vf_sync::catalog::set_state(&b.conn, "dev-b", h2.rev, "pull").unwrap();
    a.sync
        .remote
        .put(k, a.sync.keys.seal(&serde_json::to_vec(&head).unwrap(), k))
        .unwrap();
    let r = vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync);
    assert!(
        matches!(&r, Err(vf_sync::SyncError::Format(m)) if m.contains("went back")),
        "{r:?}"
    );
}

// ---- QA 2026-11-03: races and partial states (S1, S2, S4, S9) ----

use std::sync::Mutex;

use futures::stream::BoxStream;
use object_store::path::Path as OPath;
use object_store::{
    GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, PutMultipartOptions,
    PutOptions, PutPayload, PutResult,
};

type HookFn = Box<dyn FnOnce() + Send>;

/// A bucket that runs `hook` (once, on its own thread) just before the first write whose key
/// contains a given text: another workstation acting at exactly that moment.
struct Hooked {
    inner: Arc<dyn ObjectStore>,
    hook: Mutex<Option<(String, HookFn)>>,
}

impl std::fmt::Debug for Hooked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Hooked")
    }
}

impl std::fmt::Display for Hooked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Hooked")
    }
}

impl Hooked {
    fn new() -> Arc<Hooked> {
        Arc::new(Hooked {
            inner: Remote::memory(),
            hook: Mutex::new(None),
        })
    }
    fn arm(&self, contains: &str, f: HookFn) {
        *self.hook.lock().unwrap() = Some((contains.into(), f));
    }
}

#[async_trait::async_trait]
impl ObjectStore for Hooked {
    async fn put_opts(
        &self,
        location: &OPath,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        let fire = {
            let mut g = self.hook.lock().unwrap();
            if g.as_ref()
                .is_some_and(|(c, _)| location.as_ref().contains(c.as_str()))
            {
                g.take()
            } else {
                None
            }
        };
        if let Some((_, f)) = fire {
            std::thread::spawn(f).join().unwrap();
        }
        self.inner.put_opts(location, payload, opts).await
    }
    async fn put_multipart_opts(
        &self,
        location: &OPath,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.inner.put_multipart_opts(location, opts).await
    }
    async fn get_opts(
        &self,
        location: &OPath,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        self.inner.get_opts(location, options).await
    }
    async fn delete(&self, location: &OPath) -> object_store::Result<()> {
        self.inner.delete(location).await
    }
    fn list(&self, prefix: Option<&OPath>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.inner.list(prefix)
    }
    async fn list_with_delimiter(
        &self,
        prefix: Option<&OPath>,
    ) -> object_store::Result<ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }
    async fn copy(&self, from: &OPath, to: &OPath) -> object_store::Result<()> {
        self.inner.copy(from, to).await
    }
    async fn copy_if_not_exists(&self, from: &OPath, to: &OPath) -> object_store::Result<()> {
        self.inner.copy_if_not_exists(from, to).await
    }
}

fn year_of(conn: &Connection, file: &str) -> Option<i64> {
    meta(conn, &doc_id(conn, file)).3
}

fn restored(t: &Path, store: Arc<dyn ObjectStore>, cabinet_id: &str, name: &str) -> Connection {
    let mut c = catalog(&t.join(name).join("data"));
    vf_sync::restore::restore(
        &mut c,
        &t.join(name).join("Clients"),
        store,
        cabinet_id,
        PASS,
        name,
        name,
    )
    .unwrap();
    c
}

/// QA S1: A's whole push lands while B is uploading; B must not write a HEAD that drops A's
/// change.
#[test]
fn two_pushes_racing_keep_both_changes() {
    let t = tempfile::tempdir().unwrap();
    let hooked = Hooked::new();
    let store: Arc<dyn ObjectStore> = hooked.clone();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let mut b = pc_b(t.path(), store.clone(), &a);
    set_type(
        &mut a.conn,
        &a.root.clone(),
        "EIN letter.pdf",
        "W-2",
        Some(2019),
    );
    set_type(
        &mut b.conn,
        &b.root.clone(),
        "941 Q1.pdf",
        "W-2",
        Some(2018),
    );
    let (tx, rx) = std::sync::mpsc::channel();
    hooked.arm(
        "manifest/clients/",
        Box::new(move || {
            let mut a = a;
            vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
            tx.send(a).unwrap();
        }),
    );
    vf_sync::push::push(&mut b.conn, &b.root, &b.sync, None).unwrap();
    let a = rx.recv().unwrap();
    let c = restored(t.path(), store, &a.sync.cabinet_id, "c");
    assert_eq!(
        year_of(&c, "EIN letter.pdf"),
        Some(2019),
        "A's change survived"
    );
    assert_eq!(year_of(&c, "941 Q1.pdf"), Some(2018), "B's change arrived");
}

/// QA S2: an edit made while a push uploads stays pending and goes up with the next push.
#[test]
fn an_edit_during_a_push_is_sent_next_time() {
    let t = tempfile::tempdir().unwrap();
    let hooked = Hooked::new();
    let store: Arc<dyn ObjectStore> = hooked.clone();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Income Tax/2025/1099 new.pdf",
        "1099 new invented",
    );
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root.clone(), "dev-a").unwrap();
    let (root, db) = (
        a.root.clone(),
        t.path().join("a").join("data").join("catalog.db"),
    );
    hooked.arm(
        "/objects/",
        Box::new(move || {
            let mut c2 = vf_core::db::open(&db, Storage::Local).unwrap();
            set_type(&mut c2, &root, "EIN letter.pdf", "W-2", Some(2017));
        }),
    );
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert!(
        !vf_sync::catalog::pending(&a.conn, &vf_sync::rules::Rules::load(&a.root).unwrap())
            .unwrap()
            .is_empty(),
        "the edit made during the push is still pending"
    );
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let c = restored(t.path(), store, &a.sync.cabinet_id, "c");
    assert_eq!(year_of(&c, "EIN letter.pdf"), Some(2017));
}

/// QA S4: a settings file changed on two PCs is never lost silently; legal holds merge.
#[test]
fn master_files_changed_on_two_pcs() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    std::fs::write(a.root.join("_Vibe/extract_aliases.json"), r#"{"base":1}"#).unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let mut b = pc_b(t.path(), store.clone(), &a);
    std::fs::write(a.root.join("_Vibe/extract_aliases.json"), r#"{"from_a":1}"#).unwrap();
    // Two holds placed on two PCs at the same time.
    let ein = doc_id(&a.conn, "EIN letter.pdf");
    vf_fs::holds::place(
        &mut a.conn,
        &a.root,
        "dev-a",
        vf_fs::holds::Target::Docs(vec![ein]),
        "subpoena A",
    )
    .unwrap();
    let q1 = doc_id(&b.conn, "941 Q1.pdf");
    vf_fs::holds::place(
        &mut b.conn,
        &b.root,
        "dev-b",
        vf_fs::holds::Target::Docs(vec![q1]),
        "subpoena B",
    )
    .unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    std::fs::write(b.root.join("_Vibe/extract_aliases.json"), r#"{"from_b":1}"#).unwrap();
    vf_sync::push::push(&mut b.conn, &b.root, &b.sync, None).unwrap();
    let holds = vf_fs::holds::load(&b.root).unwrap();
    assert_eq!(holds.len(), 2, "both holds kept");
    let kept: Vec<String> = std::fs::read_dir(b.root.join("_Vibe/conflicts"))
        .unwrap()
        .flatten()
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert!(kept.iter().any(|t| t.contains("from_a")), "{kept:?}");
    vf_sync::pull::pull(&mut a.conn, &a.root, &a.sync).unwrap();
    assert_eq!(vf_fs::holds::load(&a.root).unwrap().len(), 2);
}

/// Removing the admin password reaches the other place, by the button or by deleting the file
/// (the forgotten-password reset); the old password never comes back (owner 2026-11-04).
#[test]
fn a_removed_admin_password_stays_removed_everywhere() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    vf_fs::trash::set_pin(&mut a.conn, &a.root, "dev-a", None, "482193").unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let mut b = pc_b(t.path(), store.clone(), &a);
    assert!(vf_fs::trash::has_pin(&b.root), "the password reached B");
    // B forgets it and deletes the file.
    std::fs::remove_file(vf_fs::trash::pin_file(&b.root)).unwrap();
    vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync).unwrap();
    assert!(!vf_fs::trash::has_pin(&b.root), "not restored by the pull");
    vf_sync::push::push(&mut b.conn, &b.root, &b.sync, None).unwrap();
    vf_sync::pull::pull(&mut a.conn, &a.root, &a.sync).unwrap();
    assert!(!vf_fs::trash::has_pin(&a.root), "the removal reached A");
    // A sets a new one: it reaches B.
    vf_fs::trash::set_pin(&mut a.conn, &a.root, "dev-a", None, "555555").unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync).unwrap();
    assert!(vf_fs::trash::check_pin(&b.root, "555555"));
}

/// QA S9: a metadata edit made here survives the other PC's new content.
#[test]
fn a_local_metadata_edit_survives_remote_content() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let mut b = pc_b(t.path(), store.clone(), &a);
    let id = doc_id(&b.conn, "EIN letter.pdf");
    let tmp = vf_fs::content::tmp_file(&b.root, "part").unwrap();
    std::fs::write(&tmp, "ein v2 invented").unwrap();
    vf_fs::content::replace(
        &mut b.conn,
        &b.root.clone(),
        "dev-b",
        &id,
        &tmp,
        vf_fs::content::Change {
            label: "edit",
            created_by: "x",
            reason: "x",
            detail: serde_json::json!({}),
            extra: vec![],
            based_on: None,
        },
    )
    .unwrap()
    .unwrap();
    vf_sync::push::push(&mut b.conn, &b.root, &b.sync, None).unwrap();
    set_type(
        &mut a.conn,
        &a.root.clone(),
        "EIN letter.pdf",
        "W-2",
        Some(2016),
    );
    vf_sync::pull::pull(&mut a.conn, &a.root, &a.sync).unwrap();
    assert_eq!(year_of(&a.conn, "EIN letter.pdf"), Some(2016));
    assert_eq!(
        std::fs::read_to_string(
            a.root
                .join("Anderson Farms LLC (1042)/Permanent/Setup/EIN letter.pdf")
        )
        .unwrap(),
        "ein v2 invented"
    );
    assert!(
        !vf_sync::catalog::pending(&a.conn, &vf_sync::rules::Rules::load(&a.root).unwrap())
            .unwrap()
            .is_empty()
    );
}

/// PA-34: many changed clients go up in rounds, each its own HEAD; another PC gets them all.
#[test]
fn a_push_in_rounds_commits_each_and_delivers_everything() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    let r = vf_sync::push::push_in_rounds(&mut a.conn, &a.root, &a.sync, None, 1).unwrap();
    assert!(r.head_rev >= 2, "one HEAD per round: {r:?}");
    assert!(r.segments >= 3, "both clients and the cabinet: {r:?}");
    assert!(r.objects >= 3, "{r:?}");
    assert!(
        vf_sync::catalog::pending(&a.conn, &vf_sync::rules::Rules::load(&a.root).unwrap())
            .unwrap()
            .is_empty()
    );
    // Nothing left: a second push sends nothing.
    let again = vf_sync::push::push_in_rounds(&mut a.conn, &a.root, &a.sync, None, 1).unwrap();
    assert_eq!((again.objects, again.segments), (0, 0), "{again:?}");
    let b = pc_b(t.path(), store.clone(), &a);
    assert_eq!(snapshot(&b.root), snapshot(&a.root));
}

/// PA-41: a client pushed from a computer with its own copy of the cabinet: refused while the
/// bucket's copy of that client came from elsewhere (nothing is written here); once it has
/// that copy, the client goes up alone, another client's change stays in the bucket, and the
/// rev this computer has seen stays where it was.
#[test]
fn a_client_pushed_alone_sends_only_that_client() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let mut b = pc_b(t.path(), store.clone(), &a);
    let anderson: String = b
        .conn
        .query_row("SELECT id FROM client WHERE ext_id = '1042'", [], |r| {
            r.get(0)
        })
        .unwrap();

    // A sends a document for Anderson; B files one too.
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Income Tax/2025/1099 A.pdf",
        "a 1099",
    );
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    put(
        &b.root,
        "Anderson Farms LLC (1042)/Income Tax/2025/K-1 B.pdf",
        "b k1",
    );
    vf_fs::rebuild::rebuild(&mut b.conn, &b.root, "dev-b").unwrap();
    let before = snapshot(&b.root);
    let err = vf_sync::push::push_client(&mut b.conn, &b.root, &b.sync, &anderson, &[])
        .unwrap_err()
        .to_string();
    assert!(err.contains("OFFICE-PC"), "{err}");
    assert_eq!(snapshot(&b.root), before, "nothing written");
    assert!(
        vf_sync::catalog::pending(&b.conn, &vf_sync::rules::Rules::load(&b.root).unwrap())
            .unwrap()
            .contains_key(&anderson)
    );

    // B brings everything in; then A changes Bell, and B pushes Anderson alone.
    vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync).unwrap();
    let seen = vf_sync::catalog::last_rev(&b.conn, "dev-b").unwrap();
    put(
        &a.root,
        "Bell Ranch Inc (2002)/Payroll/2025/941 Q2.pdf",
        "b q2",
    );
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let r = vf_sync::push::push_client(&mut b.conn, &b.root, &b.sync, &anderson, &[]).unwrap();
    assert_eq!(r.documents, 4, "Anderson's segment: {r:?}");
    assert!(r.pulled.is_none(), "{r:?}");
    assert!(
        !b.root
            .join("Bell Ranch Inc (2002)/Payroll/2025/941 Q2.pdf")
            .exists(),
        "another client's change waits for a full pull"
    );
    assert_eq!(vf_sync::catalog::last_rev(&b.conn, "dev-b").unwrap(), seen);
    assert!(
        !vf_sync::catalog::pending(&b.conn, &vf_sync::rules::Rules::load(&b.root).unwrap())
            .unwrap()
            .contains_key(&anderson)
    );

    // The main computer takes it in; a full pull there brings the rest.
    vf_sync::pull::pull(&mut a.conn, &a.root, &a.sync).unwrap();
    assert!(
        a.root
            .join("Anderson Farms LLC (1042)/Income Tax/2025/K-1 B.pdf")
            .is_file()
    );
    vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync).unwrap();
    assert_eq!(snapshot(&a.root), snapshot(&b.root));
}

/// QA of PA-41, two catalogs over one folder. (1) A document the main server filed and sent
/// that the second server has not read yet is carried over, never dropped from the bucket;
/// (3) a new file where a deleted document was stays a new document (the tombstone is not
/// given to it); (2) a pull never makes a second live row for a file another row already is.
#[test]
fn a_second_server_push_keeps_what_it_has_not_read() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    // Server B: indexed the folder on its own (no shared ids, no base).
    let mut b_conn = catalog(&t.path().join("server-b"));
    vf_fs::rebuild::rebuild(&mut b_conn, &a.root, "dev-b").unwrap();
    let b_sync = Sync::unlock(&a.root, store.clone(), PASS, "dev-b", "SERVER-2").unwrap();

    // A deletes the EIN letter and files a K-1, and sends both; then a new EIN letter is put
    // where the old one was.
    let ein = "Anderson Farms LLC (1042)/Permanent/Setup/EIN letter.pdf";
    let k1 = "Anderson Farms LLC (1042)/Income Tax/2025/K-1 main.pdf";
    let old_ein = doc_id(&a.conn, "EIN letter.pdf");
    vf_fs::trash::delete_as(
        &mut a.conn,
        &a.root,
        "dev-a",
        std::slice::from_ref(&old_ein),
        "test",
        "test",
    )
    .unwrap();
    put(&a.root, k1, "k1 main");
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    put(&a.root, ein, "ein new");
    // B has read the new EIN letter, not the K-1 (its change log not read yet).
    vf_fs::rebuild::rebuild_folders(
        &mut b_conn,
        &a.root,
        "dev-b",
        &["Anderson Farms LLC (1042)".to_string()],
    )
    .ok();
    b_conn
        .execute("DELETE FROM document WHERE rel_path = ?1", [k1])
        .unwrap();
    let anderson: String = b_conn
        .query_row("SELECT id FROM client WHERE ext_id = '1042'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let new_ein = doc_id(&b_conn, "EIN letter.pdf");
    assert_ne!(new_ein, old_ein);
    let before = snapshot(&a.root);
    vf_sync::push::push_client(
        &mut b_conn,
        &a.root,
        &b_sync,
        &anderson,
        &["OFFICE-PC".into()],
    )
    .unwrap();
    assert_eq!(snapshot(&a.root), before, "the folder is as it was");
    assert_eq!(
        doc_id(&b_conn, "EIN letter.pdf"),
        new_ein,
        "not the deleted one's id"
    );

    // What the bucket holds: the K-1 still, the new EIN letter as a new document.
    let c = restored(t.path(), store.clone(), &a.sync.cabinet_id, "bare");
    let bare = t.path().join("bare").join("Clients");
    assert!(
        bare.join(k1).is_file(),
        "the K-1 B had not read is still in the bucket"
    );
    assert_eq!(std::fs::read_to_string(bare.join(ein)).unwrap(), "ein new");
    drop(c);

    // A pulls without reading the folder again: one live row per file, no copy.
    vf_sync::pull::pull(&mut a.conn, &a.root, &a.sync).unwrap();
    let doubled: i64 = a
        .conn
        .query_row(
            "SELECT count(*) FROM (SELECT rel_path FROM document WHERE deleted_at IS NULL
             GROUP BY rel_path HAVING count(*) > 1)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(doubled, 0, "two live rows for one file");
    assert!(
        snapshot(&a.root)
            .keys()
            .all(|k| !k.contains("(conflict from") && !k.contains(" (2)")),
        "{:?}",
        snapshot(&a.root).keys().collect::<Vec<_>>()
    );
}

/// PA-41 on the real layout: two catalogs over ONE cabinet folder (several servers). The
/// second started from a copy of the main one's catalog that is now old (a seed), or was built
/// from the folder on its own; the main one pushed since. A client pushed from the second
/// changes nothing in the folder (no conflict copies, no older content put back), and the main
/// one's next pull, before it has read the folder again, adds nothing either.
fn second_server_push(seeded: bool) {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let b_dir = t.path().join("server-b");
    std::fs::create_dir_all(&b_dir).unwrap();
    if seeded {
        a.conn
            .execute(
                "VACUUM INTO ?1",
                [b_dir.join("catalog.db").display().to_string()],
            )
            .unwrap();
    }
    let mut b_conn = catalog(&b_dir);
    if !seeded {
        vf_fs::rebuild::rebuild(&mut b_conn, &a.root, "dev-b").unwrap();
    }
    let b_sync = Sync::unlock(&a.root, store.clone(), PASS, "dev-b", "SERVER-2").unwrap();

    // Since: the W-2 edited and pushed by A, then edited again (not pushed yet); a 1099 filed
    // and pushed by A, then edited; the EIN letter edited; a K-1 filed (on B, say).
    let w2 = "Anderson Farms LLC (1042)/Income Tax/2025/W-2 Northwind.pdf";
    let f1099 = "Anderson Farms LLC (1042)/Income Tax/2025/1099 A.pdf";
    put(&a.root, w2, "w2 v2");
    put(&a.root, f1099, "1099 v1");
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    put(&a.root, w2, "w2 v3");
    put(&a.root, f1099, "1099 v2");
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Permanent/Setup/EIN letter.pdf",
        "ein v2",
    );
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Income Tax/2025/K-1 new.pdf",
        "k1 new",
    );
    let before = snapshot(&a.root);

    // B reads the folder (what the change logs make it do), then pushes Anderson alone.
    vf_fs::rebuild::rebuild(&mut b_conn, &a.root, "dev-b").unwrap();
    let anderson: String = b_conn
        .query_row("SELECT id FROM client WHERE ext_id = '1042'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let r = vf_sync::push::push_client(
        &mut b_conn,
        &a.root,
        &b_sync,
        &anderson,
        &["OFFICE-PC".into()],
    )
    .unwrap();
    assert_eq!(
        r.pulled.as_ref().map(|p| p.conflicts).unwrap_or(0),
        0,
        "{r:?}"
    );
    assert_eq!(snapshot(&a.root), before, "the folder is as it was");
    assert!(
        !vf_sync::catalog::pending(&b_conn, &vf_sync::rules::Rules::load(&a.root).unwrap())
            .unwrap()
            .contains_key(&anderson)
    );

    // A pulls before reading the folder again: nothing written, nothing doubled.
    let pulled = vf_sync::pull::pull(&mut a.conn, &a.root, &a.sync).unwrap();
    assert_eq!(pulled.conflicts, 0, "{pulled:?}");
    assert_eq!(snapshot(&a.root), before, "the folder is as it was");
    let docs: i64 = a
        .conn
        .query_row(
            "SELECT count(*) FROM document WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // W-2, EIN, 941, 1099, K-1.
    assert_eq!(docs, 5);
    // Then its own full push: still nothing to change.
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    let pushed = vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert_eq!(pushed.pulled.map(|p| p.conflicts).unwrap_or(0), 0);
    assert_eq!(snapshot(&a.root), before);
    // What the bucket holds is the folder: a bare PC restores the same files.
    let c = restored(t.path(), store, &a.sync.cabinet_id, "bare");
    drop(c);
    let bare = t.path().join("bare").join("Clients");
    let mut want = before.clone();
    want.retain(|k, _| !k.starts_with("versions/"));
    let mut got = snapshot(&bare);
    got.retain(|k, _| !k.starts_with("versions/"));
    assert_eq!(got, want);
}

#[test]
fn a_client_pushed_from_a_seeded_second_server_leaves_the_shared_folder_alone() {
    second_server_push(true);
}

#[test]
fn a_client_pushed_from_a_second_server_indexed_alone_leaves_the_shared_folder_alone() {
    second_server_push(false);
}

/// What goes to B2 (owner 2026-11-12, PA-53): a document never sent outside the rule (an old
/// tax year; no year outside a Permanent folder; a Never send client) is held back, not
/// pending; widening the rule sends it; once in B2 it keeps syncing; a held document whose
/// exchange base was lost stays in the bucket's segment.
#[test]
fn the_b2_rule_holds_back_old_years() {
    use vf_sync::rules::Rules;
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store);
    // Tax years 2023 and later, whatever today's date.
    let this_year = chrono::Datelike::year(&chrono::Local::now());
    let back = u16::try_from(this_year - 2023).unwrap();
    vf_fs::clients::set_b2_years_back(&a.root, Some(back)).unwrap();
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Income Tax/2015/Old 1040.pdf",
        "old return invented",
    );
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Other/notes.pdf",
        "no year invented",
    );
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    set_type(&mut a.conn, &a.root, "Old 1040.pdf", "W-2", Some(2015));
    set_type(&mut a.conn, &a.root, "notes.pdf", "W-2", None);
    let anderson: String = a
        .conn
        .query_row("SELECT id FROM client WHERE ext_id = '1042'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let rules = Rules::load(&a.root).unwrap();
    let (held, bytes) = vf_sync::catalog::held(&a.conn, &rules).unwrap();
    assert_eq!(
        held, 2,
        "the 2015 return and the file with no year outside Permanent"
    );
    assert!(bytes > 0);

    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert!(
        vf_sync::catalog::pending(&a.conn, &rules)
            .unwrap()
            .is_empty()
    );
    let old = doc_id(&a.conn, "Old 1040.pdf");
    let sent = vf_sync::catalog::sent_ids(&a.conn).unwrap();
    assert!(!sent.contains(&old), "held back");
    assert!(
        sent.contains(&doc_id(&a.conn, "EIN letter.pdf")),
        "Permanent goes"
    );
    let out = t.path().join("export1");
    let n = vf_sync::restore::export_client(&a.sync, &anderson, &out).unwrap();
    assert_eq!(n, 2, "the 2025 W-2 and the EIN letter only");

    // Every year: both go up.
    vf_fs::clients::set_b2_years_back(&a.root, Some(0)).unwrap();
    let all = Rules::load(&a.root).unwrap();
    assert_eq!(
        vf_sync::catalog::pending(&a.conn, &all)
            .unwrap()
            .get(&anderson),
        Some(&2)
    );
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert!(vf_sync::catalog::sent_ids(&a.conn).unwrap().contains(&old));

    // Back to 2023 and later: the 2015 return is in B2 now, so an edit still goes up.
    vf_fs::clients::set_b2_years_back(&a.root, Some(back)).unwrap();
    let rules = Rules::load(&a.root).unwrap();
    set_type(&mut a.conn, &a.root, "Old 1040.pdf", "W-2", Some(2014));
    assert_eq!(
        vf_sync::catalog::pending(&a.conn, &rules)
            .unwrap()
            .get(&anderson),
        Some(&1)
    );
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert!(
        vf_sync::catalog::pending(&a.conn, &rules)
            .unwrap()
            .is_empty()
    );

    // Its exchange base lost (a catalog rebuilt): the next push keeps it in the segment.
    a.conn
        .execute("DELETE FROM sync_base WHERE doc_id = ?1", [&old])
        .unwrap();
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Income Tax/2025/1099 new.pdf",
        "new invented",
    );
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let out = t.path().join("export2");
    let n = vf_sync::restore::export_client(&a.sync, &anderson, &out).unwrap();
    assert_eq!(
        n, 5,
        "nothing taken out of B2: W-2, EIN, the 2015 return, notes, the new 1099"
    );

    // Never send: a new recent file of that client is held back.
    vf_fs::clients::set_b2_choice(&a.root, &anderson, Some("never")).unwrap();
    let rules = Rules::load(&a.root).unwrap();
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Income Tax/2026/K-1.pdf",
        "k1 invented",
    );
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    assert!(
        !vf_sync::catalog::pending(&a.conn, &rules)
            .unwrap()
            .contains_key(&anderson)
    );
    assert_eq!(vf_sync::catalog::held(&a.conn, &rules).unwrap().0, 1);

    // A conflict copy (this PC's edit of a document in B2) goes up whatever the rule says.
    let k1 = doc_id(&a.conn, "K-1.pdf");
    a.conn
        .execute(
            "INSERT INTO sync_conflict (id, doc_id, copy_doc_id, kind, from_device, at)
             VALUES ('x1', ?1, ?2, 'both_changed', 'dev-b', '2026-11-12T00:00:00Z')",
            [&old, &k1],
        )
        .unwrap();
    assert_eq!(vf_sync::catalog::held(&a.conn, &rules).unwrap().0, 0);
    assert!(
        vf_sync::catalog::pending(&a.conn, &rules)
            .unwrap()
            .contains_key(&anderson)
    );
    assert!(vf_sync::catalog::sent_ids(&a.conn).unwrap().contains(&k1));

    // The rule cannot be read: the push stops, never "send everything".
    std::fs::write(a.root.join("_Vibe").join("clients.vf.json"), "{ not json").unwrap();
    assert!(Rules::load(&a.root).is_err());
    assert!(vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).is_err());
}

/// PA-62: documents pushed by hand go up whatever the B2 rule says, without the rest of what
/// the rule would send; they keep syncing afterwards; one outside a client folder is refused.
#[test]
fn selected_documents_are_pushed_whatever_the_rule_says() {
    use vf_sync::rules::Rules;
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store);
    let this_year = chrono::Datelike::year(&chrono::Local::now());
    let back = u16::try_from(this_year - 2023).unwrap();
    vf_fs::clients::set_b2_years_back(&a.root, Some(back)).unwrap();
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Income Tax/2015/Old 1040.pdf",
        "old return invented",
    );
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Other/notes.pdf",
        "no year invented",
    );
    put(&a.root, "loose.pdf", "outside every client invented");
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    set_type(&mut a.conn, &a.root, "Old 1040.pdf", "W-2", Some(2015));
    let anderson: String = a
        .conn
        .query_row("SELECT id FROM client WHERE ext_id = '1042'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let old = doc_id(&a.conn, "Old 1040.pdf");
    let notes = doc_id(&a.conn, "notes.pdf");
    let w2 = doc_id(&a.conn, "W-2 Northwind.pdf");

    // Nothing has ever been pushed. The 2015 return alone, though the rule holds it back.
    let r = vf_sync::push::push_docs(
        &mut a.conn,
        &a.root,
        &a.sync,
        std::slice::from_ref(&old),
        &[],
    )
    .unwrap();
    assert_eq!((r.segments, r.objects), (1, 1), "{r:?}");
    let sent = vf_sync::catalog::sent_ids(&a.conn).unwrap();
    assert!(sent.contains(&old));
    assert!(!sent.contains(&notes), "another held document stays held");
    assert!(
        !sent.contains(&w2),
        "what the rule would send waits for a push"
    );
    let out = t.path().join("one");
    assert_eq!(
        vf_sync::restore::export_client(&a.sync, &anderson, &out).unwrap(),
        1
    );

    // The ordinary push sends the rule's documents and keeps the one pushed by hand.
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let out = t.path().join("three");
    assert_eq!(
        vf_sync::restore::export_client(&a.sync, &anderson, &out).unwrap(),
        3,
        "the 2025 W-2, the EIN letter and the 2015 return"
    );
    // It keeps syncing: an edit is pending under the rule that held it.
    let rules = Rules::load(&a.root).unwrap();
    set_type(&mut a.conn, &a.root, "Old 1040.pdf", "W-2", Some(2014));
    assert_eq!(
        vf_sync::catalog::pending(&a.conn, &rules)
            .unwrap()
            .get(&anderson),
        Some(&1)
    );
    // Pushed again by hand with a second one: both are in B2, nothing sent twice.
    let r = vf_sync::push::push_docs(
        &mut a.conn,
        &a.root,
        &a.sync,
        &[old.clone(), notes.clone()],
        &[],
    )
    .unwrap();
    assert_eq!(r.objects, 1, "only the notes file is new: {r:?}");
    assert!(
        vf_sync::catalog::sent_ids(&a.conn)
            .unwrap()
            .contains(&notes)
    );
    assert!(
        vf_sync::catalog::pending(&a.conn, &rules)
            .unwrap()
            .is_empty()
    );

    // A document outside every client folder is refused, with its name.
    a.conn
        .execute(
            "INSERT INTO document (id, rel_path, hash, size, filed_at, device_id, updated_at)
             VALUES ('loose', 'loose.pdf', ?1, 1, 't', 'dev-a', 't')",
            ["ab".repeat(32)],
        )
        .unwrap();
    let loose = "loose".to_string();
    let e = vf_sync::push::push_docs(&mut a.conn, &a.root, &a.sync, &[loose], &[]).unwrap_err();
    assert!(e.to_string().contains("loose.pdf"), "{e}");
}

/// PA-63: the firm's list of rules decides what goes up by itself, in the catalog's counts and
/// in a push alike; an empty list sends nothing; what is in B2 stays there.
/// Q-104, Q-106: a document taken out of B2 leaves the bucket only. Its file stays on every
/// computer; no push sends it again, from this catalog or from one made later; pushing it by
/// hand puts it back.
#[test]
fn a_document_taken_out_of_b2_stays_on_every_computer() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let mut b = pc_b(t.path(), store.clone(), &a);
    let w2_rel = "Anderson Farms LLC (1042)/Income Tax/2025/W-2 Northwind.pdf";
    let w2 = doc_id(&a.conn, "W-2 Northwind.pdf");
    let objects = |s: &Sync| s.remote.list("objects").unwrap().len();
    let in_segment = |s: &Sync, conn: &Connection| -> Vec<String> {
        let client: String = conn
            .query_row("SELECT id FROM client WHERE ext_id = '1042'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let head = s.head().unwrap().unwrap();
        let seg: vf_sync::layout::ClientSegment = s
            .get_json(&head.client_key(&client, head.segments[&client]))
            .unwrap()
            .unwrap();
        let mut names: Vec<String> = seg
            .documents
            .into_iter()
            .filter(|d| d.deleted_at.is_none())
            .map(|d| d.rel_path)
            .collect();
        names.sort();
        names
    };
    let before = objects(&a.sync);
    assert_eq!(in_segment(&a.sync, &a.conn).len(), 2);

    let r = vf_sync::push::remove_docs(
        &mut a.conn,
        &a.root,
        &a.sync,
        std::slice::from_ref(&w2),
        &[],
    )
    .unwrap();
    assert_eq!((r.documents, r.objects, r.waiting), (1, 1, 0));
    assert_eq!(objects(&a.sync), before - 1);
    assert_eq!(
        in_segment(&a.sync, &a.conn),
        ["Permanent/Setup/EIN letter.pdf"]
    );
    // The file and its document stay here.
    assert!(a.root.join(w2_rel).is_file());
    let live: bool = a
        .conn
        .query_row(
            "SELECT deleted_at IS NULL FROM document WHERE id = ?1",
            [&w2],
            |r| r.get(0),
        )
        .unwrap();
    assert!(live);
    assert!(vf_sync::removed::ids(&a.root).unwrap().contains(&w2));

    // And on the other computer, which pulls the client without it.
    vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync).unwrap();
    assert!(b.root.join(w2_rel).is_file(), "not deleted there");
    let live: bool = b
        .conn
        .query_row(
            "SELECT deleted_at IS NULL FROM document WHERE id = ?1",
            [&w2],
            |r| r.get(0),
        )
        .unwrap();
    assert!(live);

    // Changed afterwards, and everything pushed: it is not sent again.
    set_type(&mut a.conn, &a.root, "W-2 Northwind.pdf", "W-2", Some(2024));
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Income Tax/2025/1099 new.pdf",
        "1099 invented",
    );
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert_eq!(
        in_segment(&a.sync, &a.conn),
        [
            "Income Tax/2025/1099 new.pdf",
            "Permanent/Setup/EIN letter.pdf"
        ]
    );
    assert_eq!(objects(&a.sync), before);

    // A catalog made later for the same folder (a session host the next morning) pushes
    // the client: still not sent.
    let c_dir = t.path().join("server-c");
    let mut c_conn = catalog(&c_dir);
    vf_fs::rebuild::rebuild(&mut c_conn, &a.root, "dev-c").unwrap();
    let c_sync = Sync::unlock(&a.root, store.clone(), PASS, "dev-c", "SERVER-3").unwrap();
    let anderson: String = c_conn
        .query_row("SELECT id FROM client WHERE ext_id = '1042'", [], |r| {
            r.get(0)
        })
        .unwrap();
    vf_sync::push::push_client(
        &mut c_conn,
        &a.root,
        &c_sync,
        &anderson,
        &["OFFICE-PC".to_string()],
    )
    .unwrap();
    assert_eq!(in_segment(&c_sync, &c_conn).len(), 2);
    assert_eq!(objects(&c_sync), before);

    // Pushed by hand: back in B2, and off the list.
    vf_sync::push::push_docs(
        &mut a.conn,
        &a.root,
        &a.sync,
        std::slice::from_ref(&w2),
        &[],
    )
    .unwrap();
    assert!(vf_sync::removed::ids(&a.root).unwrap().is_empty());
    assert_eq!(in_segment(&a.sync, &a.conn).len(), 3);
    assert_eq!(objects(&a.sync), before + 1);
}

/// PA-63: a push asked to stop sends nothing more and says so; the next push goes on from
/// there, and the progress counts what went up against what was to be sent.
#[test]
fn a_stopped_push_goes_on_at_the_next_one() {
    use vf_sync::rules::Rules;
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store);
    let cabinet = a.sync.cabinet_id.clone();
    let rules = Rules::load(&a.root).unwrap();
    let (files, bytes) = vf_sync::catalog::to_send(&a.conn, &rules, None).unwrap();
    assert_eq!(files, 3);
    assert!(bytes > 0);
    let one: String = a
        .conn
        .query_row("SELECT id FROM client WHERE ext_id = '2002'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        vf_sync::catalog::to_send(&a.conn, &rules, Some(std::slice::from_ref(&one)))
            .unwrap()
            .0,
        1
    );

    vf_sync::push::begin(&cabinet, files, bytes);
    vf_sync::push::ask_stop(&cabinet);
    let r = vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert!(r.stopped);
    assert_eq!((r.objects, r.documents), (0, 0));
    assert_eq!(vf_sync::push::progress(&cabinet).files, 0);

    vf_sync::push::begin(&cabinet, files, bytes);
    let r = vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert!(!r.stopped);
    assert_eq!((r.objects, r.documents), (3, 3));
    let p = vf_sync::push::progress(&cabinet);
    assert_eq!((p.files, p.bytes), (files, bytes));
    assert_eq!(
        vf_sync::catalog::to_send(&a.conn, &rules, None).unwrap(),
        (0, 0)
    );
}

/// Q-107 with B2: a client merged into another leaves the bucket's listing; its documents
/// are listed once, under the kept client, and a restore from B2 has each of them once.
#[test]
fn a_merged_client_leaves_the_bucket_listing() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let id = |conn: &Connection, ext: &str| -> String {
        conn.query_row("SELECT id FROM client WHERE ext_id = ?1", [ext], |r| {
            r.get(0)
        })
        .unwrap()
    };
    let (kept, dup) = (id(&a.conn, "1042"), id(&a.conn, "2002"));
    assert!(a.sync.head().unwrap().unwrap().segments.contains_key(&dup));
    let plan = vf_fs::clientmerge::plan(&a.conn, &a.root, &dup, &kept).unwrap();
    vf_fs::clientmerge::apply(&mut a.conn, &a.root, &plan, "dev-a").unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let head = a.sync.head().unwrap().unwrap();
    assert!(
        !head.segments.contains_key(&dup),
        "the merged client is gone"
    );
    assert!(head.segments.contains_key(&kept));
    // Nothing left to send, and a second push changes nothing.
    let again = vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert_eq!(again.segments, 0);

    // A bare PC restores the cabinet: one client folder, each document once.
    let b = pc_b(t.path(), store, &a);
    let files = snapshot(&b.root);
    let docs: Vec<&String> = files.keys().filter(|k| k.ends_with(".pdf")).collect();
    assert_eq!(docs.len(), 3, "{docs:?}");
    assert!(
        files.contains_key("Anderson Farms LLC (1042)/Payroll/2025/941 Q1.pdf"),
        "{docs:?}"
    );
    assert!(!b.root.join("Bell Ranch Inc (2002)").exists());
    let n: i64 = b
        .conn
        .query_row(
            "SELECT count(*) FROM document WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 3);
}

/// Code review 2026-11-14: the same content filed to two clients (a K-1 to the entity and
/// the shareholder). Taking one out of B2 keeps the content in the bucket for the other,
/// also when this catalog has not read the other one's file.
#[test]
fn removing_one_of_two_same_documents_keeps_the_content_in_the_bucket() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store);
    // The W-2's bytes again, in the other client's folder.
    put(
        &a.root,
        "Bell Ranch Inc (2002)/Income Tax/2025/W-2 copy.pdf",
        "w2 2025 invented",
    );
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let objects = a.sync.remote.list("objects").unwrap().len();
    let w2 = doc_id(&a.conn, "W-2 Northwind.pdf");
    // A catalog made from the folder listing has not read the copy.
    a.conn
        .execute(
            "UPDATE document SET hash = NULL WHERE rel_path LIKE '%W-2 copy.pdf'",
            [],
        )
        .unwrap();
    let r = vf_sync::push::remove_docs(
        &mut a.conn,
        &a.root,
        &a.sync,
        std::slice::from_ref(&w2),
        &[],
    )
    .unwrap();
    assert_eq!((r.documents, r.objects), (1, 0), "the copy still uses it");
    assert_eq!(a.sync.remote.list("objects").unwrap().len(), objects);
    // The copy was read to find that out.
    let read: bool = a
        .conn
        .query_row(
            "SELECT hash IS NOT NULL FROM document WHERE rel_path LIKE '%W-2 copy.pdf'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(read);
}

#[test]
fn a_list_of_rules_decides_what_goes_up() {
    use vf_fs::clients::{B2Rule, B2Years};
    use vf_sync::rules::Rules;
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store);
    let ein = doc_id(&a.conn, "EIN letter.pdf");
    let w2 = doc_id(&a.conn, "W-2 Northwind.pdf");
    let q941 = doc_id(&a.conn, "941 Q1.pdf");
    let w2_type: String = a
        .conn
        .query_row(
            "SELECT doc_type_id FROM document WHERE id = ?1",
            [&w2],
            |r| r.get(0),
        )
        .unwrap();
    let sent = |a: &Pc| vf_sync::catalog::sent_ids(&a.conn).unwrap();
    let held = |a: &Pc| {
        vf_sync::catalog::held(&a.conn, &Rules::load(&a.root).unwrap())
            .unwrap()
            .0
    };

    // One rule: the Setup folder of the permanent file, documents with no tax year.
    vf_fs::clients::set_b2_rules(
        &a.root,
        &[B2Rule {
            name: "Setup".into(),
            folders: vec!["Permanent/Setup".into()],
            years: B2Years::None,
            no_year: true,
            ..B2Rule::default()
        }],
    )
    .unwrap();
    assert_eq!(held(&a), 2, "the W-2 and the 941 wait");
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert!(sent(&a).contains(&ein));
    assert!(!sent(&a).contains(&w2) && !sent(&a).contains(&q941));

    // A second rule, by doc type: either rule matching sends.
    let mut rules = vf_fs::clients::load_b2(&a.root).unwrap().rules.unwrap();
    rules.push(B2Rule {
        name: "W-2s".into(),
        doc_types: vec![w2_type],
        ..B2Rule::default()
    });
    vf_fs::clients::set_b2_rules(&a.root, &rules).unwrap();
    assert_eq!(held(&a), 1, "the 941 only");
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert!(sent(&a).contains(&w2));
    assert!(!sent(&a).contains(&q941));

    // No rules: nothing more goes up by itself, and nothing leaves B2.
    vf_fs::clients::set_b2_rules(&a.root, &[]).unwrap();
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Permanent/Setup/Articles.pdf",
        "articles invented",
    );
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    assert_eq!(held(&a), 2, "the 941 and the new file");
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert!(!sent(&a).contains(&doc_id(&a.conn, "Articles.pdf")));
    assert!(sent(&a).contains(&ein) && sent(&a).contains(&w2));
    // The client's own Always send still wins.
    let bell: String = a
        .conn
        .query_row("SELECT id FROM client WHERE ext_id = '2002'", [], |r| {
            r.get(0)
        })
        .unwrap();
    vf_fs::clients::set_b2_choices(&a.root, std::slice::from_ref(&bell), Some("always")).unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert!(sent(&a).contains(&q941));
}

/// PA-60: documents cataloged from the folder listing have no fingerprint until something
/// needs it. A push reads the ones it sends, and only those.
#[test]
fn a_push_fingerprints_what_it_sends_and_nothing_else() {
    use vf_fs::clients::{B2Rule, B2Years};
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store);
    // Only the permanent file goes up by itself.
    vf_fs::clients::set_b2_rules(
        &a.root,
        &[B2Rule {
            name: "Permanent".into(),
            folders: vec!["Permanent".into()],
            years: B2Years::None,
            no_year: true,
            ..B2Rule::default()
        }],
    )
    .unwrap();
    let before: Vec<(String, String)> = a
        .conn
        .prepare("SELECT rel_path, hash FROM document WHERE deleted_at IS NULL ORDER BY rel_path")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    a.conn
        .execute("UPDATE document SET hash = NULL", [])
        .unwrap();
    let hash_of = |a: &Pc, file: &str| -> Option<String> {
        a.conn
            .query_row(
                "SELECT hash FROM document WHERE rel_path LIKE '%' || ?1",
                [file],
                |r| r.get(0),
            )
            .unwrap()
    };

    let r = vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    assert_eq!(r.skipped, 0, "{r:?}");
    let ein = before
        .iter()
        .find(|(p, _)| p.ends_with("EIN letter.pdf"))
        .unwrap();
    assert_eq!(
        hash_of(&a, "EIN letter.pdf").as_ref(),
        Some(&ein.1),
        "read and sent"
    );
    assert!(
        hash_of(&a, "W-2 Northwind.pdf").is_none(),
        "held back: not read"
    );
    assert!(hash_of(&a, "941 Q1.pdf").is_none(), "held back: not read");
    assert!(
        vf_sync::catalog::sent_ids(&a.conn)
            .unwrap()
            .contains(&doc_id(&a.conn, "EIN letter.pdf"))
    );

    // Pushed by hand: read then, and sent.
    let w2 = doc_id(&a.conn, "W-2 Northwind.pdf");
    vf_sync::push::push_docs(
        &mut a.conn,
        &a.root,
        &a.sync,
        std::slice::from_ref(&w2),
        &[],
    )
    .unwrap();
    assert!(hash_of(&a, "W-2 Northwind.pdf").is_some());
    assert!(hash_of(&a, "941 Q1.pdf").is_none());
}

/// PA-80: what the portal's staff page is told is in B2, by client, from the catalog alone.
#[test]
fn the_documents_in_b2_are_listed_by_client() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    // Nothing exchanged yet: no client has anything in the bucket.
    let none = vf_sync::listed::listed(&a.conn, "dev-a").unwrap();
    assert!(none.clients.is_empty());
    assert_eq!(none.live.len(), 2);
    assert_eq!(none.head_rev, 0);

    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let l = vf_sync::listed::listed(&a.conn, "dev-a").unwrap();
    assert!(l.head_rev > 0);
    let names = |l: &vf_sync::listed::Listed, code: &str| -> Vec<String> {
        l.clients
            .iter()
            .find(|c| c.code == code)
            .map(|c| c.docs.iter().map(|d| d.path.clone()).collect())
            .unwrap_or_default()
    };
    // Below the client's folder, by path.
    assert_eq!(
        names(&l, "1042"),
        [
            "Income Tax/2025/W-2 Northwind.pdf",
            "Permanent/Setup/EIN letter.pdf"
        ]
    );
    assert_eq!(names(&l, "2002"), ["Payroll/2025/941 Q1.pdf"]);
    let anderson = l.clients.iter().find(|c| c.code == "1042").unwrap();
    assert_eq!(anderson.name, "Anderson Farms LLC");
    assert!(anderson.rev > 0);
    let w2 = &anderson.docs[0];
    assert_eq!(w2.doc_id, doc_id(&a.conn, "W-2 Northwind.pdf"));
    assert_eq!(w2.tax_year, Some(2025));
    assert_eq!(w2.size, "w2 2025 invented".len() as i64);

    // Another PC that pulled lists the same.
    let b = pc_b(t.path(), store.clone(), &a);
    let lb = vf_sync::listed::listed(&b.conn, &b.sync.device_id).unwrap();
    assert_eq!(names(&lb, "1042"), names(&l, "1042"));
    assert_eq!(
        lb.clients.iter().map(|c| c.rev).collect::<Vec<_>>(),
        l.clients.iter().map(|c| c.rev).collect::<Vec<_>>()
    );

    // A document not sent (filed since the push) is not listed; one taken out of B2 leaves.
    put(
        &a.root,
        "Anderson Farms LLC (1042)/Income Tax/2025/1099 new.pdf",
        "not sent yet",
    );
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    assert_eq!(
        names(&vf_sync::listed::listed(&a.conn, "dev-a").unwrap(), "1042").len(),
        2
    );
    let w2_id = w2.doc_id.clone();
    vf_sync::push::remove_docs(
        &mut a.conn,
        &a.root,
        &a.sync,
        std::slice::from_ref(&w2_id),
        &[],
    )
    .unwrap();
    let after = vf_sync::listed::listed(&a.conn, "dev-a").unwrap();
    assert!(!names(&after, "1042").contains(&"Income Tax/2025/W-2 Northwind.pdf".to_string()));
    let rev_after = after.clients.iter().find(|c| c.code == "1042").unwrap().rev;
    assert!(rev_after > anderson.rev, "the segment moved on");
}

/// Writes a small pushed bucket into the folder `VF_PORTAL_FIXTURE` names, with `expect.json`
/// beside it (the passphrase, the clients and each document's content): Vibe-Portal's tests
/// open it with the portal's own reader of the bucket, so the two cannot drift apart unseen.
/// Does nothing unless the variable is set.
#[test]
fn writes_a_bucket_for_the_portals_tests() {
    let Ok(dir) = std::env::var("VF_PORTAL_FIXTURE") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let t = tempfile::tempdir().unwrap();
    let store = Remote::folder(&dir.join("bucket")).unwrap();
    let mut a = pc_a(t.path(), store);
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    // One document deleted after the push, and pushed again: the manifest keeps its tombstone.
    let gone = doc_id(&a.conn, "EIN letter.pdf");
    std::fs::remove_file(
        a.root
            .join("Anderson Farms LLC (1042)/Permanent/Setup/EIN letter.pdf"),
    )
    .unwrap();
    vf_fs::rebuild::rebuild(&mut a.conn, &a.root, "dev-a").unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let l = vf_sync::listed::listed(&a.conn, "dev-a").unwrap();
    let body = |path: &str| match path {
        "Income Tax/2025/W-2 Northwind.pdf" => "w2 2025 invented",
        "Payroll/2025/941 Q1.pdf" => "941 invented",
        _ => "",
    };
    let clients: Vec<serde_json::Value> = l
        .clients
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id,
                "code": c.code,
                "name": c.name,
                "docs": c.docs.iter().map(|d| serde_json::json!({
                    "docId": d.doc_id,
                    "path": d.path,
                    "size": d.size,
                    "taxYear": d.tax_year,
                    "content": body(&d.path),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let expect = serde_json::json!({
        "passphrase": PASS,
        "cabinetId": a.sync.cabinet_id,
        "deletedDocId": gone,
        "clients": clients,
    });
    std::fs::write(
        dir.join("expect.json"),
        serde_json::to_vec_pretty(&expect).unwrap(),
    )
    .unwrap();
}

/// QA 2026-11-17: an admin password file that is there and cannot be read is not "deleted
/// here". The pull stops and says so; the removal is not written or sent up.
#[test]
fn an_unreadable_admin_password_file_is_not_a_removal() {
    let t = tempfile::tempdir().unwrap();
    let store = Remote::memory();
    let mut a = pc_a(t.path(), store.clone());
    vf_fs::trash::set_pin(&mut a.conn, &a.root, "dev-a", None, "482193").unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let mut b = pc_b(t.path(), store.clone(), &a);
    // Unreadable on B: a folder where the file is.
    let pin = vf_fs::trash::pin_file(&b.root);
    let text = std::fs::read(&pin).unwrap();
    std::fs::remove_file(&pin).unwrap();
    std::fs::create_dir(&pin).unwrap();
    // Something new in the bucket, so B's pull takes the master files.
    std::fs::write(a.root.join("_Vibe/extract_aliases.json"), r#"{"from_a":1}"#).unwrap();
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let e = vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync).unwrap_err();
    assert!(e.to_string().contains("cannot be read"), "{e}");
    assert!(pin.is_dir(), "nothing was written in its place");
    // Readable again: the password is as it was, on B and (after B's push) on A.
    std::fs::remove_dir(&pin).unwrap();
    std::fs::write(&pin, text).unwrap();
    vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync).unwrap();
    vf_sync::push::push(&mut b.conn, &b.root, &b.sync, None).unwrap();
    vf_sync::pull::pull(&mut a.conn, &a.root, &a.sync).unwrap();
    assert!(vf_fs::trash::check_pin(&a.root, "482193"));
    assert!(vf_fs::trash::check_pin(&b.root, "482193"));
}
