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
    // PA-22: the government copy row keeps the printed name (no rename set).
    let name = "Anderson Farms LLC_ANDE1042_Anderson Farms LLC_2025_1040_GovernmentCopyTaxReturn_ANDE1042.pdf";
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

    // Filer: the exact ID picks the client; the managed rule for the row picks the folder.
    let master = vf_fs::clients::master_dir(&root);
    vf_ingest::parsers::sync_filer_rules(&root).unwrap();
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
    assert_eq!(hit.rule_name, "UltraTax CS · GovernmentCopyTaxReturn");
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
    assert!(
        rel.ends_with("/Anderson Farms LLC_2025_1040_GovernmentCopyTaxReturn_ANDE1042.pdf"),
        "filed under the printed name: {rel}"
    );
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

    let p = |n: &str| format!("Anderson Farms LLC_2025_{n}_ANDE1042.pdf");
    let dest = |n: &str| vf_ingest::parsers::trial(&root, &p(n)).destination.unwrap();
    vf_ingest::parsers::sync_filer_rules(&root).unwrap();
    assert!(
        !vf_ingest::parsers::sync_filer_rules(&root).unwrap(),
        "stable"
    );

    // Not in the document table: the [[doc]] rules and the doc type's folder (PA-21).
    let tr = vf_ingest::parsers::trial(&root, &p("1040_Engagement Letter"));
    assert_eq!(tr.rule, "Engagement letter · UltraTax CS");
    assert!(dest("1040_Engagement Letter").starts_with("Correspondence/2025/"));

    // The table (PA-22): each copy by its own row, the longer name never taken by the shorter.
    for (doc, rule, folder) in [
        (
            "1040_Tax_Return",
            "UltraTax CS · ArchivedTaxReturn",
            "Income Tax/2025",
        ),
        (
            "1040_Preparer_Copy_Tax_Return",
            "UltraTax CS · PreparerCopyTaxReturn",
            "Workpapers & Support/2025",
        ),
        (
            "1040_GovernmentCopyTaxReturn",
            "UltraTax CS · GovernmentCopyTaxReturn",
            "Income Tax/2025/Government Copy",
        ),
        (
            "1040_Tax_Return_Documents",
            "UltraTax CS · TaxSourceDocuments",
            "Workpapers & Support/2025",
        ),
        (
            "1040_Amended_Tax_Return",
            "UltraTax CS · ArchivedTaxReturn",
            "Income Tax/2025",
        ),
    ] {
        let t = vf_ingest::parsers::trial(&root, &p(doc));
        assert_eq!(t.rule, rule, "{doc}");
        assert_eq!(
            t.destination.unwrap(),
            format!("{folder}/{}", p(doc)),
            "{doc}"
        );
    }

    // The short standard names (owner 2026-11-04: either, chosen per profile).
    let (files, _) = vf_ingest::parsers::load_files(&root);
    let (path, mut prof) = files.into_iter().next().unwrap();
    prof.short_names = true;
    vf_ingest::parsers::save_profile(&path, &prof).unwrap();
    vf_ingest::parsers::sync_filer_rules(&root).unwrap();
    assert_eq!(
        dest("1040_Tax_Return"),
        "Income Tax/2025/2025 1040 Tax_Return.pdf"
    );
    assert_eq!(
        dest("1040_Amended_Tax_Return"),
        "Income Tax/2025/2025 1040 Amended Tax_Return.pdf"
    );
    assert_eq!(
        dest("1120S_K1_Package_Pat Partner"),
        "Income Tax/2025/2025 1120S K1_Package - Pat Partner.pdf"
    );
    prof.short_names = false;
    vf_ingest::parsers::save_profile(&path, &prof).unwrap();
    vf_ingest::parsers::sync_filer_rules(&root).unwrap();

    // Government copies to their own folder and renamed; preparer copies not filed.
    let (files, errors) = vf_ingest::parsers::load_files(&root);
    assert!(errors.is_empty(), "{errors:?}");
    let (path, mut prof) = files.into_iter().next().unwrap();
    for r in &mut prof.documents {
        match r.default_name.as_str() {
            "GovernmentCopyTaxReturn" => {
                r.folder = "Other/{Year}".into();
                r.rename = "{Year} {Form} {Document} - {Client}".into();
            }
            "PreparerCopyTaxReturn" => r.action = vf_ingest::ultratax::Action::Skip,
            _ => {}
        }
    }
    vf_ingest::parsers::save_profile(&path, &prof).unwrap();
    assert_eq!(
        vf_ingest::parsers::load_files(&root).0[0].1.documents,
        prof.documents
    );
    assert!(vf_ingest::parsers::sync_filer_rules(&root).unwrap());
    assert_eq!(
        dest("1040_GovernmentCopyTaxReturn"),
        "Other/2025/2025 1040 GovernmentCopyTaxReturn - Anderson Farms LLC.pdf"
    );
    let t = vf_ingest::parsers::trial(&root, &p("1040_Preparer_Copy_Tax_Return"));
    assert!(t.destination.unwrap_err().contains("not filed"));
    let (inbox, capture) = (t_dir(&root, "inbox"), t_dir(&root, "capture"));
    let printed = capture.join(p("1040_Preparer_Copy_Tax_Return"));
    filer_pdf::pdf::write_test_pdf(&printed, &["synthetic preparer copy"]);
    let ctx = Context {
        root: &root,
        inbox: &inbox,
        device_id: "dev",
        pdfium: None,
        ocr: None,
    };
    let done = pipeline::process(&ctx, &printed, Origin::PrintCapture).unwrap();
    assert!(done.delivered.is_empty(), "not sent to Intake");
    assert!(std::fs::read_dir(&inbox).unwrap().next().is_none());
    assert!(done.kept.is_file() && done.kept.to_string_lossy().contains("_processed"));
    // A rename without the document name is refused (Intake could not recognize it).
    let mut bad = prof.clone();
    bad.documents[0].rename = "{Year} return".into();
    assert!(vf_ingest::parsers::save_profile(&path, &bad).is_err());

    // A K-1 for one partner: its row's rule carries the copy to the partner.
    let (rules, _, _) = filer_core::rules::load_or_seed(&master);
    let k1 = rules
        .active_rules()
        .iter()
        .find(|r| r.name == "UltraTax CS · ArchiveK1Package")
        .unwrap();
    assert_eq!(k1.copies.len(), 1);
    assert!(
        rules.active_rules()[0]
            .id
            .starts_with(vf_ingest::parsers::MANAGED_PREFIX),
        "the table goes ahead of Filer's K-1 rules"
    );
    let compiled = filer_core::rules::CompiledRules::new(rules.active_rules()).unwrap();
    let rest = "Anderson Farms LLC_2025_1120S_K1_Package_Pat Partner_ANDE1042";
    assert_eq!(
        compiled.first_match(rest).name,
        "UltraTax CS · ArchiveK1Package"
    );
    assert_eq!(compiled.captures(&k1.id, rest)["who"], "Pat Partner");

    // QA2 E1/E2: a year in the client's name does not move the file to that year; a document
    // name inside a client's name, or an ordinary inbox file, is not taken by a row.
    assert_eq!(
        vf_ingest::parsers::trial(&root, "Smith 2019 Trust_2025_1041_Tax_Return_ANDE1042.pdf")
            .destination
            .unwrap(),
        "Income Tax/2025/2025 Smith 2019 Trust_2025_1041_Tax_Return_ANDE1042.pdf"
    );
    let t = vf_ingest::parsers::trial(
        &root,
        "Tax Return Pros LLC_2025_1120S_InputScreens_ANDE1042.pdf",
    );
    assert_eq!(t.rule, "UltraTax CS · InputScreens");
    assert!(
        !compiled
            .first_match("2025 Tax Return signed")
            .id
            .starts_with(vf_ingest::parsers::MANAGED_PREFIX),
        "a hand-dropped file keeps Filer's own rules"
    );

    // QA2 U1: a save that would give a bad rule is refused before anything is written.
    let mut bad = prof.clone();
    bad.documents[0].folder = "Income Tax/CON".into();
    assert!(vf_ingest::parsers::check_profile(&root, &path, &bad).is_err());
    // QA2 U6: a profile file that does not read leaves Intake's rules as they are.
    let rules_before = std::fs::read(filer_core::rules::path(&master)).unwrap();
    std::fs::write(vf_ingest::parsers::dir(&root).join("broken.toml"), "name = ").unwrap();
    assert!(vf_ingest::parsers::sync_filer_rules_report(&root).is_err());
    assert_eq!(std::fs::read(filer_core::rules::path(&master)).unwrap(), rules_before);
    std::fs::remove_file(vf_ingest::parsers::dir(&root).join("broken.toml")).unwrap();

    // A disabled profile takes its rules out again; Filer's own rules stay.
    prof.enabled = false;
    vf_ingest::parsers::save_profile(&path, &prof).unwrap();
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

/// A folder beside the cabinet (outside it, as staging and inboxes must be).
fn t_dir(root: &Path, name: &str) -> std::path::PathBuf {
    let d = root.parent().unwrap().join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}
