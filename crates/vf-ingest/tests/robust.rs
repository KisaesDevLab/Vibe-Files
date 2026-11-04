//! QA batch 6: delivery never replaces a file; a file the app stopped on is set aside.

use vf_ingest::pipeline;
use vf_ingest::worker::{self, Event};

/// QA I9: a name that is taken gets ` (2)`, and the file there is untouched.
#[test]
fn rename_free_never_replaces() {
    let t = tempfile::tempdir().unwrap();
    let (a, b) = (t.path().join("a.tmp"), t.path().join("out"));
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(&a, "new").unwrap();
    std::fs::write(b.join("x.pdf"), "old").unwrap();
    let got = pipeline::rename_free(&a, &b, "x.pdf").unwrap();
    assert_eq!(got, b.join("x (2).pdf"));
    assert_eq!(std::fs::read_to_string(b.join("x.pdf")).unwrap(), "old");
    assert_eq!(std::fs::read_to_string(&got).unwrap(), "new");
    assert!(!a.exists());
}

/// QA I4 / QA2 #1: one stop on a file (a quit) tries it again; two in a row set it aside.
#[test]
fn a_file_the_app_stopped_on_twice_is_set_aside() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path();
    let m = worker::marker(dir, "dev-1");
    std::fs::write(dir.join("bad.pdf"), "x").unwrap();
    std::fs::write(&m, "bad.pdf	1").unwrap();
    assert!(
        worker::recover_marker(dir, "dev-1").is_none(),
        "one stop: tried again"
    );
    assert!(dir.join("bad.pdf").exists());
    assert!(
        worker::recover_marker(dir, "dev-2").is_none(),
        "another PC's marker"
    );
    std::fs::write(&m, "bad.pdf	2").unwrap();
    let ev = worker::recover_marker(dir, "dev-1").expect("set aside");
    assert!(matches!(ev, Event::Failed { .. }));
    assert!(!dir.join("bad.pdf").exists());
    assert!(!m.exists());
    let failed = dir.join(pipeline::FAILED_DIR);
    let day = std::fs::read_dir(&failed)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(day.join("bad.pdf").is_file());
    assert!(day.join("bad.pdf.error.txt").is_file());
    // A marker whose file is gone is just removed.
    std::fs::write(&m, "gone.pdf	3").unwrap();
    assert!(worker::recover_marker(dir, "dev-1").is_none());
    assert!(!m.exists());
}

/// QA2 #3: the handled list survives a restart while the file is unchanged.
#[test]
fn handled_files_stay_handled_until_they_change() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path();
    let f = dir.join("open.pdf");
    std::fs::write(&f, "x").unwrap();
    let m = std::fs::metadata(&f).unwrap();
    let secs = m
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(
        dir.join(worker::handled_file(dir, "dev-1")),
        format!("open.pdf	{}	{secs}", m.len()),
    )
    .unwrap();
    assert!(worker::load_handled(dir, "dev-1").contains_key(&f));
    std::fs::write(&f, "changed").unwrap();
    assert!(worker::load_handled(dir, "dev-1").is_empty());
}
