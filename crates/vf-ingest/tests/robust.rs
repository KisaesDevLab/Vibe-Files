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

/// QA I4: the marker left by a run that died on a file sets that file aside once.
#[test]
fn a_file_the_app_stopped_on_is_set_aside() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path();
    std::fs::write(dir.join("bad.pdf"), "x").unwrap();
    std::fs::write(dir.join(worker::MARKER), "bad.pdf").unwrap();
    let ev = worker::recover_marker(dir).expect("set aside");
    assert!(matches!(ev, Event::Failed { .. }));
    assert!(!dir.join("bad.pdf").exists());
    assert!(!dir.join(worker::MARKER).exists());
    let failed = dir.join(pipeline::FAILED_DIR);
    let day = std::fs::read_dir(&failed)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(day.join("bad.pdf").is_file());
    assert!(day.join("bad.pdf.error.txt").is_file());
    // Nothing named: nothing happens.
    assert!(worker::recover_marker(dir).is_none());
    // A marker whose file is gone is just removed.
    std::fs::write(dir.join(worker::MARKER), "gone.pdf").unwrap();
    assert!(worker::recover_marker(dir).is_none());
    assert!(!dir.join(worker::MARKER).exists());
}
