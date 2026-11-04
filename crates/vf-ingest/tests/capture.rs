//! PHASES P02 done-when: a PDF saved by a PDF printer into the capture folder with an
//! UltraTax-style name is filed to the client's year folder with doc type
//! "Return copy — federal" and source firm.

use std::path::Path;
use std::sync::atomic::AtomicUsize;

use filer_intake::commit::{CommitInput, CommitItem, run_commit};
use filer_intake::plan::FolderAction;
use vf_core::db::Storage;
use vf_ingest::pipeline::{self, Context, Origin};

#[test]
fn ultratax_print_lands_as_a_federal_return_copy() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("cabinet");
    let firm = uuid::Uuid::new_v4();
    vf_fs::cabinet::create(
        &root,
        &vf_fs::cabinet::CabinetFile {
            cabinet_id: vf_core::new_id(),
            firm_id: firm.to_string(),
            schema_version: vf_core::db::SCHEMA_VERSION,
            layout_version: 1,
            created: vf_core::now_rfc3339(),
            name: None,
        },
    )
    .unwrap();
    let mut conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
    vf_fs::doctypes::seed_if_missing(&root).unwrap();
    vf_fs::doctypes::sync(&mut conn, &root, "dev", "seed").unwrap();
    let csv = t.path().join("clients.csv");
    std::fs::write(&csv, "ID,Name\nANDE1042,Anderson Farms LLC\n").unwrap();
    let map = filer_core::config::ColumnMap {
        id: Some("ID".into()),
        name: Some("Name".into()),
        extras: vec![],
    };
    let list = vf_fs::clients::import_csv(&root, &csv, &map, &Default::default()).unwrap();

    let (inbox, capture) = (t.path().join("inbox"), t.path().join("capture"));
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::create_dir_all(&capture).unwrap();
    // The office's UltraTax shape (owner, 2026-11-03), with an invented client.
    let printed = capture.join("Anderson Farms LLC_2025_1040_GovernmentCopyTaxReturn_ANDE1042.pdf");
    filer_pdf::pdf::write_test_pdf(&printed, &["Form 1040 2025 synthetic return"]);

    let ctx = Context {
        root: &root,
        inbox: &inbox,
        device_id: "dev",
        pdfium: None,
        ocr: None,
    };
    let done = pipeline::process(&ctx, &printed, Origin::PrintCapture).unwrap();
    let name = "Anderson Farms LLC_ANDE1042_2025 UltraTax CS Return copy — federal 1040 Government Copy Tax Return.pdf";
    assert_eq!(done.delivered[0].path, inbox.join(name));
    assert_eq!(
        done.delivered[0].note,
        "UltraTax CS filename profile · Return copy — federal"
    );
    assert!(
        vf_ingest::parsers::dir(&root)
            .join("ultratax.toml")
            .is_file(),
        "seeded"
    );

    // Filer: the exact ID picks the client; the firm's rules pick the folder.
    let master = vf_fs::clients::master_dir(&root);
    let (rules, _, _) = filer_core::rules::load_or_seed(&master);
    let compiled = filer_core::rules::CompiledRules::new(rules.active_rules()).unwrap();
    // UltraTax IDs here are letters + digits: Filer's ID pattern is set to match them.
    let re = filer_core::parse::filename_regex(r"[A-Z]{4}\d{4}").unwrap();
    let parsed = filer_core::parse::parse_filename(name, &re, 2026);
    let client = list
        .match_by_id(parsed.id.as_deref().unwrap())
        .unwrap()
        .clone();
    let hit = filer_core::rules::evaluate(&compiled, &parsed, &client, parsed.year);
    assert_eq!(hit.rule_name, "UltraTax");
    let rendered = hit.outcome.unwrap();
    let folder = root.join("Anderson Farms LLC (ANDE1042)");
    let dest = folder.join(&rendered.dest_rel).join(&rendered.filename);
    let rep = run_commit(
        &CommitInput {
            batch: uuid::Uuid::new_v4(),
            items: vec![CommitItem {
                src: done.delivered[0].path.clone(),
                dest: dest.clone(),
                folder: FolderAction::WillCreate(folder.clone()),
                client,
                overwrite: false,
                source: filer_intake::intake::Source::Inbox(inbox.clone()),
                merged_from: vec![],
                split_of: None,
                copies: vec![],
                kept_both: false,
                also_dirs: vec![],
            }],
            skipped: vec![],
            root: root.clone(),
            first_inbox: Some(inbox.clone()),
            data_dir: master,
            firm_id: firm,
            template: filer_core::template::default_tree(),
            splits: vec![],
            force_copy: false,
        },
        &AtomicUsize::new(0),
    );
    assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    assert!(dest.is_file());

    vf_fs::intake::reconcile(&mut conn, &root, "dev").unwrap();
    let (doc_type, year, source, rel): (String, i64, String, String) = conn
        .query_row(
            "SELECT t.name, d.tax_year, d.source, d.rel_path FROM document d
             JOIN doc_type t ON t.id = d.doc_type_id WHERE d.deleted_at IS NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(doc_type, "Return copy — federal");
    assert_eq!((year, source.as_str()), (2025, "firm"));
    assert!(rel.starts_with("Anderson Farms LLC (ANDE1042)/"), "{rel}");
    assert!(rel.contains("/Income Tax/2025/"), "year folder: {rel}");
    let _ = Path::new(&rel);
}

/// Admin › Print capture (option 3, owner 2026-11-03): each mapped doc type's prints are filed
/// in its own folder: the doc type's default, or the profile's override.
#[test]
fn ultratax_prints_go_to_their_doc_types_folder() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("cabinet");
    vf_fs::cabinet::create(
        &root,
        &vf_fs::cabinet::CabinetFile {
            cabinet_id: vf_core::new_id(),
            firm_id: uuid::Uuid::new_v4().to_string(),
            schema_version: vf_core::db::SCHEMA_VERSION,
            layout_version: 1,
            created: vf_core::now_rfc3339(),
            name: None,
        },
    )
    .unwrap();
    vf_fs::doctypes::seed_if_missing(&root).unwrap();
    let csv = t.path().join("clients.csv");
    std::fs::write(&csv, "ID,Name\nANDE1042,Anderson Farms LLC\n").unwrap();
    let map = filer_core::config::ColumnMap {
        id: Some("ID".into()),
        name: Some("Name".into()),
        extras: vec![],
    };
    vf_fs::clients::import_csv(&root, &csv, &map, &Default::default()).unwrap();
    let master = vf_fs::clients::master_dir(&root);
    let mut s = filer_intake::config::Settings::load_or_default(&master).0;
    s.id_pattern = "[A-Z0-9]{4,}".into();
    s.save(&master).unwrap();
    vf_ingest::parsers::seed_if_missing(&root).unwrap();

    let engagement = "Anderson Farms LLC_2025_1040_Engagement Letter_ANDE1042.pdf";
    let federal = "Anderson Farms LLC_2025_1040_GovernmentCopyTaxReturn_ANDE1042.pdf";
    let dest = |n: &str| vf_ingest::parsers::trial(&root, n).destination.unwrap();
    // Before: Filer's UltraTax rule sends everything to Income Tax.
    assert!(
        dest(engagement).starts_with("Income Tax/2025/"),
        "{}",
        dest(engagement)
    );

    assert!(vf_ingest::parsers::sync_filer_rules(&root).unwrap());
    assert!(
        !vf_ingest::parsers::sync_filer_rules(&root).unwrap(),
        "stable"
    );
    let tr = vf_ingest::parsers::trial(&root, engagement);
    assert_eq!(tr.rule, "Engagement letter · UltraTax CS");
    assert!(
        dest(engagement).starts_with("Correspondence/2025/"),
        "{}",
        dest(engagement)
    );
    assert!(
        dest(federal).starts_with("Income Tax/2025/"),
        "{}",
        dest(federal)
    );

    // An override in the profile wins over the doc type's folder.
    let (files, errors) = vf_ingest::parsers::load_files(&root);
    assert!(errors.is_empty(), "{errors:?}");
    let (path, mut p) = files.into_iter().next().unwrap();
    p.folders
        .insert("Return copy — federal".into(), "Other/{Year}".into());
    vf_ingest::parsers::save_profile(&path, &p).unwrap();
    assert_eq!(
        vf_ingest::parsers::load_files(&root).0[0].1,
        p,
        "round trip"
    );
    assert!(vf_ingest::parsers::sync_filer_rules(&root).unwrap());
    assert!(
        dest(federal).starts_with("Other/2025/"),
        "{}",
        dest(federal)
    );

    // Filer's own rules stay; a disabled profile takes its rules out again.
    p.enabled = false;
    vf_ingest::parsers::save_profile(&path, &p).unwrap();
    assert!(vf_ingest::parsers::sync_filer_rules(&root).unwrap());
    let (rules, _, _) = filer_core::rules::load_or_seed(&master);
    assert!(
        rules
            .active_rules()
            .iter()
            .all(|r| !r.id.starts_with(vf_ingest::parsers::MANAGED_PREFIX))
    );
    assert!(rules.active_rules().iter().any(|r| r.name == "UltraTax"));
}
