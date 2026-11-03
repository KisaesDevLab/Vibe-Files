//! PHASES P02 done-when: a PDF saved by a PDF printer into the capture folder with an
//! UltraTax-style name is filed to the client's year folder with doc type
//! "Return copy — federal" and source firm.

use std::path::Path;
use std::sync::atomic::AtomicUsize;

use filer_intake::commit::{CommitInput, CommitItem, run_commit};
use filer_intake::plan::FolderAction;
use vf_core::db::{CatalogKey, Storage};
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
    let mut conn =
        vf_core::db::open(&t.path().join("c.db"), &CatalogKey([7; 32]), Storage::Local).unwrap();
    vf_fs::doctypes::seed_if_missing(&root).unwrap();
    vf_fs::doctypes::sync(&mut conn, &root, "dev", "seed").unwrap();
    let csv = t.path().join("clients.csv");
    std::fs::write(&csv, "ID,Name\n1042,Anderson Farms LLC\n").unwrap();
    let map = filer_core::config::ColumnMap {
        id: Some("ID".into()),
        name: Some("Name".into()),
        extras: vec![],
    };
    let list = vf_fs::clients::import_csv(&root, &csv, &map, &Default::default()).unwrap();

    let (inbox, capture) = (t.path().join("inbox"), t.path().join("capture"));
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::create_dir_all(&capture).unwrap();
    let printed = capture.join("1042_2025_Client Copy.pdf");
    filer_pdf::pdf::write_test_pdf(&printed, &["Form 1040 2025 synthetic return"]);

    let ctx = Context {
        root: &root,
        inbox: &inbox,
        device_id: "dev",
        pdfium: None,
    };
    let done = pipeline::process(&ctx, &printed, Origin::PrintCapture).unwrap();
    let name = "Anderson Farms LLC_1042_2025 UltraTax CS Return copy — federal.pdf";
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
    let re = filer_core::parse::filename_regex(filer_core::parse::DEFAULT_ID_PATTERN).unwrap();
    let parsed = filer_core::parse::parse_filename(name, &re, 2026);
    let client = list
        .match_by_id(parsed.id.as_deref().unwrap())
        .unwrap()
        .clone();
    let hit = filer_core::rules::evaluate(&compiled, &parsed, &client, parsed.year);
    assert_eq!(hit.rule_name, "UltraTax");
    let rendered = hit.outcome.unwrap();
    let folder = root.join("Anderson Farms LLC (1042)");
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
    assert!(rel.starts_with("Anderson Farms LLC (1042)/"), "{rel}");
    assert!(rel.contains("/Income Tax/2025/"), "year folder: {rel}");
    let _ = Path::new(&rel);
}
