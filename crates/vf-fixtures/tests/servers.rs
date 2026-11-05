//! PA-36: two Remote Desktop servers on one cabinet, each with its own catalog. A change made
//! on one reaches the other through the cabinet's change logs.

use std::path::Path;

use vf_core::db::{Connection, Storage};
use vf_fs::docmeta::MetaEdit;

fn catalog(p: &Path) -> Connection {
    vf_core::db::open(p, Storage::Local).unwrap()
}

fn doc_type_of(conn: &Connection, id: &str) -> Option<String> {
    conn.query_row(
        "SELECT t.name FROM document d LEFT JOIN doc_type t ON t.id = d.doc_type_id WHERE d.id = ?1",
        [id],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn an_edit_on_one_server_reaches_the_other() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("Clients");
    vf_fixtures::generate(&root, 5).unwrap();
    vf_fs::doctypes::seed_if_missing(&root).unwrap();
    let mut a = catalog(&t.path().join("a.db"));
    vf_fs::doctypes::sync(&mut a, &root, "dev-a", "seed").unwrap();
    vf_fs::rebuild::rebuild(&mut a, &root, "dev-a").unwrap();
    // Server B starts from a copy of A's catalog (seeding), so ids match.
    a.execute(
        "VACUUM INTO ?1",
        [t.path().join("b.db").to_string_lossy().as_ref()],
    )
    .unwrap();
    let mut b = catalog(&t.path().join("b.db"));
    // B's first look marks where the logs end.
    assert!(
        vf_fs::changes::catch_up(&mut b, &root, "server-b", "dev-b")
            .unwrap()
            .is_empty()
    );

    // On A: a document gets a doc type (noted in A's change log).
    let (id, folder): (String, String) = a
        .query_row(
            "SELECT d.id, c.folder_rel FROM document d JOIN client c ON c.id = d.client_id
             WHERE d.deleted_at IS NULL ORDER BY d.rel_path LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let w2: String = a
        .query_row("SELECT id FROM doc_type WHERE name = 'W-2'", [], |r| {
            r.get(0)
        })
        .unwrap();
    vf_fs::changes::set_writer(&root, "server-a");
    vf_fs::docmeta::edit(
        &mut a,
        &root,
        std::slice::from_ref(&id),
        &MetaEdit {
            doc_type: Some(Some(w2)),
            ..Default::default()
        },
        "dev-a",
    )
    .unwrap();
    vf_fs::changes::clear_writer();
    assert_eq!(doc_type_of(&a, &id).as_deref(), Some("W-2"));
    assert_ne!(
        doc_type_of(&b, &id).as_deref(),
        Some("W-2"),
        "B does not know yet"
    );

    // B catches up: that client's folder is re-read, and the doc type comes over.
    let got = vf_fs::changes::catch_up(&mut b, &root, "server-b", "dev-b").unwrap();
    assert_eq!(got, [folder]);
    assert_eq!(doc_type_of(&b, &id).as_deref(), Some("W-2"));
    // Nothing new the second time.
    assert!(
        vf_fs::changes::catch_up(&mut b, &root, "server-b", "dev-b")
            .unwrap()
            .is_empty()
    );
}
