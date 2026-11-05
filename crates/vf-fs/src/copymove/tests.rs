use std::collections::BTreeMap;

use filer_core::clients::Client;
use uuid::Uuid;
use vf_core::db::Storage;

use super::*;
use crate::cabinet::CabinetFile;

struct Cab {
    root: PathBuf,
    conn: Connection,
}

fn put(p: &Path, body: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// A cabinet with two invented clients, indexed.
fn cabinet(dir: &Path) -> Cab {
    let root = dir.join("cabinet");
    let firm = Uuid::new_v4();
    crate::cabinet::create(
        &root,
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
    for (name, id) in [("Acme LLC", "1001"), ("Baker Co", "1002")] {
        let folder = root.join(format!("{name} ({id})"));
        std::fs::create_dir_all(folder.join("Income Tax/2025")).unwrap();
        filer_intake::sentinel::write_sentinel(
            &root,
            &folder,
            &Client {
                id: id.into(),
                uuid: Uuid::new_v4(),
                name: name.into(),
                extra: BTreeMap::new(),
            },
            firm,
        )
        .unwrap();
    }
    put(
        &root.join("Acme LLC (1001)/Income Tax/2025/k1.pdf"),
        "k-1 invented",
    );
    crate::doctypes::seed_if_missing(&root).unwrap();
    let mut conn = vf_core::db::open(&dir.join("catalog.db"), Storage::Local).unwrap();
    crate::rebuild::rebuild(&mut conn, &root, "dev").unwrap();
    Cab { root, conn }
}

fn doc(c: &Connection, like: &str) -> Option<(String, Option<String>, Option<String>)> {
    c.query_row(
        "SELECT d.id, d.client_id, t.name FROM document d LEFT JOIN doc_type t ON t.id = d.doc_type_id
         WHERE d.deleted_at IS NULL AND d.rel_path LIKE ?1",
        [like],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .ok()
}

fn set_type(c: &mut Cab, id: &str) {
    let w2: String = c
        .conn
        .query_row("SELECT id FROM doc_type WHERE name = 'W-2'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let root = c.root.clone();
    crate::docmeta::edit(
        &mut c.conn,
        &root,
        &[id.to_string()],
        &crate::docmeta::MetaEdit {
            doc_type: Some(Some(w2)),
            ..Default::default()
        },
        "dev",
    )
    .unwrap();
}

#[test]
fn copy_to_another_client_keeps_the_metadata_and_undo_removes_it() {
    let t = tempfile::tempdir().unwrap();
    let mut c = cabinet(t.path());
    let (id, acme, _) = doc(&c.conn, "Acme%k1.pdf").unwrap();
    set_type(&mut c, &id);
    let dest = Dest {
        root: c.root.clone(),
        folder: "Baker Co (1002)/Income Tax/2025".into(),
    };
    let rows = plan(
        &c.conn,
        &c.root,
        std::slice::from_ref(&id),
        &dest,
        Mode::Copy,
    )
    .unwrap();
    let root = c.root.clone();
    let (tx, n) = commit(&mut c.conn, &root, &rows, &dest, Mode::Copy, "dev").unwrap();
    assert_eq!(n, 1);
    // Both there; the copy is Baker's, with the doc type.
    let (_, a_client, _) = doc(&c.conn, "Acme%k1.pdf").unwrap();
    assert_eq!(a_client, acme);
    let (copy, b_client, b_type) = doc(&c.conn, "Baker%k1.pdf").unwrap();
    assert_ne!(copy, id);
    assert_ne!(b_client, acme);
    assert_eq!(b_type.as_deref(), Some("W-2"));
    assert!(
        c.root
            .join("Baker Co (1002)/Income Tax/2025/k1.pdf")
            .is_file()
    );
    // Undo: the copy is gone, the original stays.
    crate::fileops::undo(&mut c.conn, &root, &tx, "dev").unwrap();
    assert!(doc(&c.conn, "Baker%k1.pdf").is_none());
    assert!(
        !c.root
            .join("Baker Co (1002)/Income Tax/2025/k1.pdf")
            .exists()
    );
    assert!(doc(&c.conn, "Acme%k1.pdf").is_some());
}

#[test]
fn move_to_another_client_and_back_by_undo() {
    let t = tempfile::tempdir().unwrap();
    let mut c = cabinet(t.path());
    let (id, acme, _) = doc(&c.conn, "Acme%k1.pdf").unwrap();
    let dest = Dest {
        root: c.root.clone(),
        folder: "Baker Co (1002)/Income Tax".into(),
    };
    let rows = plan(
        &c.conn,
        &c.root,
        std::slice::from_ref(&id),
        &dest,
        Mode::Move,
    )
    .unwrap();
    let root = c.root.clone();
    let (tx, _) = commit(&mut c.conn, &root, &rows, &dest, Mode::Move, "dev").unwrap();
    let (same, baker, _) = doc(&c.conn, "Baker%k1.pdf").unwrap();
    assert_eq!(same, id, "the same document, moved");
    assert_ne!(baker, acme);
    assert!(doc(&c.conn, "Acme%k1.pdf").is_none());
    crate::fileops::undo(&mut c.conn, &root, &tx, "dev").unwrap();
    let (_, back, _) = doc(&c.conn, "Acme%k1.pdf").unwrap();
    assert_eq!(back, acme);
    assert!(
        c.root
            .join("Acme LLC (1001)/Income Tax/2025/k1.pdf")
            .is_file()
    );
}

#[test]
fn move_to_another_cabinet_carries_the_metadata() {
    let t = tempfile::tempdir().unwrap();
    let mut a = cabinet(&t.path().join("a"));
    let mut b = cabinet(&t.path().join("b"));
    let (id, _, _) = doc(&a.conn, "Acme%k1.pdf").unwrap();
    set_type(&mut a, &id);
    // A taken name in the other cabinet: keep both.
    let dest = Dest {
        root: b.root.clone(),
        folder: "Acme LLC (1001)/Income Tax/2025".into(),
    };
    let rows = plan(
        &a.conn,
        &a.root,
        std::slice::from_ref(&id),
        &dest,
        Mode::Move,
    )
    .unwrap();
    assert!(matches!(&rows[0].outcome, Outcome::KeepBoth(r) if r.ends_with("k1 (2).pdf")));
    let root = a.root.clone();
    commit(&mut a.conn, &root, &rows, &dest, Mode::Move, "dev").unwrap();
    assert!(doc(&a.conn, "Acme%k1.pdf").is_none(), "moved away");
    assert!(
        !a.root
            .join("Acme LLC (1001)/Income Tax/2025/k1.pdf")
            .exists()
    );
    // The other cabinet finds it with its doc type.
    crate::rebuild::rebuild(&mut b.conn, &dest.root, "dev").unwrap();
    let (_, _, ty) = doc(&b.conn, "%k1 (2).pdf").unwrap();
    assert_eq!(ty.as_deref(), Some("W-2"));
    // Not into a folder that is no client's.
    let bad = Dest {
        root: b.root.clone(),
        folder: "_Vibe".into(),
    };
    assert!(plan(&b.conn, &b.root, &[], &bad, Mode::Copy).is_err());
    assert_eq!(client_folders(&b.root).len(), 2);
    assert!(
        folders_in(&b.root, "Baker Co (1002)")
            .contains(&"Baker Co (1002)/Income Tax/2025".to_string())
    );
}
