//! PHASES P11 done-when 4: a bare PC with the bucket's credentials and the sync passphrase
//! restores the fixture cabinet (`vf_fixtures::generate`: five clients, Filer sentinels, three
//! years of synthetic PDFs) to byte-identical files. In-memory bucket.

use std::collections::BTreeMap;
use std::path::Path;

use vf_core::db::Storage;

fn files(root: &Path) -> BTreeMap<String, String> {
    vf_fs::rebuild::cabinet_files(root)
        .into_iter()
        .filter(|f| !f.ends_with(vf_fixtures::MARKER))
        .map(|f| {
            (
                vf_fs::rebuild::rel(root, &f),
                filer_intake::commit::sha256_file(&f).unwrap(),
            )
        })
        .collect()
}

#[test]
fn the_fixture_cabinet_restores_byte_for_byte() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("office").join("Clients");
    let summary = vf_fixtures::generate(&a, 7).unwrap();
    let mut conn = vf_core::db::open(&t.path().join("office").join("catalog.db"), Storage::Local)
        .unwrap();
    vf_fs::rebuild::rebuild(&mut conn, &a, "dev-a").unwrap();
    let cab = vf_fs::cabinet::read(&a).unwrap();
    let store = vf_sync::Remote::memory();
    let remote = vf_sync::Remote::new(store.clone(), &cab.cabinet_id);
    let fast = vf_security::keys::KdfParams {
        m_kib: 256,
        t: 1,
        p: 1,
    };
    let keys = vf_sync::setup::init(&a, &remote, "correct horse battery staple", fast).unwrap();
    let s = vf_sync::Sync {
        remote,
        keys,
        cabinet_id: cab.cabinet_id.clone(),
        device_id: "dev-a".into(),
        device_name: "OFFICE".into(),
    };
    let r = vf_sync::push::push(&mut conn, &a, &s, None).unwrap();
    assert_eq!(r.documents, summary.docs.len());

    let b = t.path().join("bare").join("Clients");
    std::fs::create_dir_all(t.path().join("bare")).unwrap();
    let mut conn_b =
        vf_core::db::open(&t.path().join("bare").join("catalog.db"), Storage::Local).unwrap();
    let rep = vf_sync::restore::restore(
        &mut conn_b,
        &b,
        store,
        &cab.cabinet_id,
        "correct horse battery staple",
        "dev-b",
        "BARE",
    )
    .unwrap();
    assert_eq!(rep.clients, summary.clients);
    assert_eq!(rep.documents, summary.docs.len());
    assert_eq!(files(&b), files(&a), "every file byte-identical");
    for f in vf_fs::cabinet::client_folders(&a).unwrap() {
        let name = f.file_name().unwrap();
        assert_eq!(
            std::fs::read(filer_intake::sentinel::sentinel_path(&f)).unwrap(),
            std::fs::read(filer_intake::sentinel::sentinel_path(&b.join(name))).unwrap(),
            "same sentinel bytes, same folder name"
        );
    }
    let n: i64 = conn_b
        .query_row(
            "SELECT count(*) FROM document WHERE deleted_at IS NULL AND client_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n as usize, summary.docs.len());
}
