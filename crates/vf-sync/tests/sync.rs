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
    m_kib: 256,
    t: 1,
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
    assert!(vf_sync::catalog::pending(&a.conn).unwrap().is_empty());
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
    assert_eq!(vf_sync::catalog::pending(&a.conn).unwrap().len(), 1);
    vf_sync::push::push(&mut a.conn, &a.root, &a.sync, None).unwrap();
    let p = vf_sync::pull::pull(&mut b.conn, &b.root, &b.sync).unwrap();
    assert_eq!(p.new, 1);
    let id = doc_id(&a.conn, "1099-INT Lakeside.pdf");
    assert_eq!(meta(&b.conn, &id), meta(&a.conn, &id));
    assert_eq!(meta(&b.conn, &id).2.as_deref(), Some("1099-INT"));
    // B pulled, so it has nothing to push; and a second pull changes nothing.
    assert!(vf_sync::catalog::pending(&b.conn).unwrap().is_empty());
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
    let p = vf_sync::catalog::pending(&a.conn).unwrap();
    assert_eq!(p.get(&anderson), Some(&4));
    assert_eq!(p.get(&bell), Some(&1));
    let r = vf_sync::push::push(&mut a.conn, &a.root, &a.sync, Some(&anderson)).unwrap();
    assert_eq!(r.objects, 4);
    let p = vf_sync::catalog::pending(&a.conn).unwrap();
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
        !vf_sync::catalog::pending(&a.conn).unwrap().is_empty(),
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
    assert!(!vf_sync::catalog::pending(&a.conn).unwrap().is_empty());
}
