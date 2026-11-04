//! PHASES P10 done-when: a FileCenter-style `Cabinet/Drawer/Folder` tree of 2,000 files imports
//! with ≥ 90% auto-mapped and a reviewable remainder. All names are invented.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use vf_core::db::{Connection, Storage};
use vf_fs::cabinet::CabinetFile;
use vf_fs::import::{self, Gap, Options};

const FIRST: [&str; 12] = [
    "Dana", "Sam", "Pat", "Jordan", "Jamie", "Avery", "Riley", "Morgan", "Casey", "Quinn", "Drew",
    "Reese",
];
const LAST: [&str; 12] = [
    "Halvorsen",
    "Okafor",
    "Lindqvist",
    "Marchetti",
    "Delacroix",
    "Nakamura",
    "Brennan",
    "Szabo",
    "Fairweather",
    "Castellano",
    "Whitlock",
    "Abernathy",
];
const BUSINESS: [&str; 8] = [
    "Harbor Point Farms LLC",
    "Copper Kettle Bakery Inc",
    "Bluestem Logistics LLC",
    "Willow Creek Dental PC",
    "Iron Gate Storage LP",
    "Prairie Sky Partners",
    "Redwood Valley Trust",
    "Summit Lane Holdings Inc",
];

/// Folders inside a drawer and the files they hold; `{y}` is the folder's year.
const YEAR_FOLDERS: [&str; 4] = ["{y}", "{y} Tax Return", "TY{y}", "Tax Year {y}"];
const SOURCE_FILES: [&str; 18] = [
    "W2 Acme Corp.pdf",
    "W-2 Northwind.pdf",
    "1099-INT Lakeside Bank.pdf",
    "1099 DIV Summit Brokerage.pdf",
    "1099-R Pension.pdf",
    "1098 Mortgage Interest.pdf",
    "K-1 1065 Riverbend.pdf",
    "Organizer.pdf",
    "8879 signed.pdf",
    "Form 1040 Client Copy.pdf",
    "Extension 4868.pdf",
    "Estimated Tax Vouchers.pdf",
    "Property Tax Bill County.pdf",
    "Charitable Donations.pdf",
    "Brokerage Statement Dec.pdf",
    "SSA-1099.pdf",
    "Depreciation Schedule.xlsx",
    "Workpapers.xlsx",
];
const NOISE_FILES: [&str; 3] = ["scan0001.pdf", "Doc12.pdf", "misc notes.pdf"];
const CATEGORY_FOLDERS: [(&str, &[&str]); 4] = [
    (
        "Correspondence {y}",
        &[
            "IRS Notice CP2000.pdf",
            "Engagement Letter.pdf",
            "Letter to client.pdf",
        ],
    ),
    (
        "Bank Statements {y}",
        &["January.pdf", "February.pdf", "March.pdf", "April.pdf"],
    ),
    (
        "Payroll {y}",
        &["941 Q1.pdf", "941 Q2.pdf", "Payroll Register.pdf"],
    ),
    (
        "Permanent",
        &[
            "EIN Letter CP575.pdf",
            "Articles of Organization.pdf",
            "Form 2553 S Election.pdf",
            "Operating Agreement.pdf",
        ],
    ),
];

/// A file's tag in letters, so it never reads as a form number or a year.
fn tag(n: usize) -> String {
    let mut n = n;
    let mut s = String::from("x");
    loop {
        s.push((b'a' + (n % 26) as u8) as char);
        n /= 26;
        if n == 0 {
            return s;
        }
    }
}

/// The doc type each invented file name should get (`None`: none).
fn expected(name: &str) -> Option<&'static str> {
    let table: [(&str, &str); 34] = [
        ("misc notes", "Misc"),
        ("W2 Acme", "W-2"),
        ("W-2 North", "W-2"),
        ("1099-INT", "1099-INT"),
        ("1099 DIV", "1099-DIV"),
        ("1099-R", "1099-R"),
        ("1098 Mort", "1098"),
        ("K-1 1065", "K-1 (1065)"),
        ("Organizer", "Organizer"),
        ("8879", "8879 e-file authorization"),
        ("Form 1040", "Return copy — federal"),
        ("Extension", "Extension"),
        ("Estimated", "Estimate vouchers"),
        ("Property Tax", "Property tax statement"),
        ("Charitable", "Charitable receipt"),
        ("Brokerage Statement", "Brokerage statement"),
        ("SSA-1099", "SSA-1099"),
        ("Depreciation", "Depreciation schedule"),
        ("Workpapers", "Workpaper"),
        ("CP2000", "Notice / correspondence"),
        ("Engagement", "Engagement letter"),
        ("Letter to client", "Notice / correspondence"),
        ("January", "Bank statement"),
        ("February", "Bank statement"),
        ("March", "Bank statement"),
        ("April", "Bank statement"),
        ("941 Q", "Payroll tax return"),
        ("Payroll Register", "Payroll report"),
        ("EIN Letter", "EIN letter"),
        ("Articles", "Formation documents"),
        ("2553", "S election (2553)"),
        ("Operating", "Operating agreement"),
        ("scan0001", ""),
        ("README", ""),
    ];
    table
        .iter()
        .find(|(k, _)| name.contains(k))
        .map(|(_, v)| *v)
        .filter(|v| !v.is_empty())
}

fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

struct Fx {
    _t: tempfile::TempDir,
    root: PathBuf,
    conn: Connection,
    files: usize,
}

/// The client list (CSV) and a drawer per client, in the naming styles legacy trees use, plus
/// drawers no client matches and loose files.
fn tree(target: usize) -> Fx {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("FileCenter").join("Clients");
    std::fs::create_dir_all(&root).unwrap();
    vf_fs::cabinet::create(
        &root,
        &CabinetFile {
            cabinet_id: vf_core::new_id(),
            firm_id: uuid::Uuid::new_v4().to_string(),
            schema_version: vf_core::db::SCHEMA_VERSION,
            layout_version: 1,
            created: vf_core::now_rfc3339(),
            name: Some("Clients".into()),
        },
    )
    .unwrap();
    let mut conn = vf_core::db::open(&t.path().join("catalog.db"), Storage::Local).unwrap();
    vf_fs::doctypes::seed_if_missing(&root).unwrap();
    vf_fs::doctypes::sync(&mut conn, &root, "dev", "seed").unwrap();

    // Clients: people as `Last, First` in the CSV; businesses as they are.
    let mut clients: Vec<(String, String)> = vec![];
    for (i, l) in LAST.iter().enumerate() {
        for (j, f) in FIRST.iter().enumerate().filter(|(j, _)| (i + j) % 3 == 0) {
            clients.push((
                format!("{}{:02}{:02}", l[..3].to_uppercase(), i, j),
                format!("{l}, {f}"),
            ));
        }
    }
    for (i, b) in BUSINESS.iter().enumerate() {
        clients.push((format!("BUS{:04}", 100 + i), b.to_string()));
    }
    let csv = t.path().join("clients.csv");
    let mut text = String::from("ID,Name\n");
    for (id, name) in &clients {
        text.push_str(&format!("{id},\"{name}\"\n"));
    }
    std::fs::write(&csv, text).unwrap();
    let map = filer_core::config::ColumnMap {
        id: Some("ID".into()),
        name: Some("Name".into()),
        extras: vec![],
    };
    vf_fs::clients::import_csv(&root, &csv, &map, &Default::default()).unwrap();

    // Drawers: `Last, First`, `First Last`, `Name (ID)`, the business name.
    let mut drawers: Vec<String> = clients
        .iter()
        .enumerate()
        .map(|(k, (id, name))| match (k % 4, name.split_once(", ")) {
            (0, _) => format!("{name} ({id})"),
            (1, Some((l, f))) => format!("{f} {l}"),
            _ => name.clone(),
        })
        .collect();
    drawers.push("Old Clients - Misc".into());
    drawers.push("Scans to sort".into());

    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut files = 0;
    let write = |p: &Path, files: &mut usize| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("invented content {}", *files)).unwrap();
        *files += 1;
    };
    'fill: loop {
        for d in &drawers {
            let years = [2021, 2022, 2023];
            let y = years[(rng(&mut seed) % 3) as usize];
            let yf = YEAR_FOLDERS[(rng(&mut seed) % 4) as usize].replace("{y}", &y.to_string());
            for _ in 0..4 {
                let r = rng(&mut seed);
                let name = if r.is_multiple_of(20) {
                    NOISE_FILES[(r / 20 % 3) as usize]
                } else {
                    SOURCE_FILES[(r / 20 % SOURCE_FILES.len() as u64) as usize]
                };
                let p = root
                    .join(d)
                    .join(&yf)
                    .join(format!("{} {name}", tag(files)));
                write(&p, &mut files);
            }
            let (cf, names) = CATEGORY_FOLDERS[(rng(&mut seed) % 4) as usize];
            let cf = cf.replace("{y}", &y.to_string());
            let name = names[(rng(&mut seed) % names.len() as u64) as usize];
            write(
                &root
                    .join(d)
                    .join(&cf)
                    .join(format!("{} {name}", tag(files))),
                &mut files,
            );
            if files >= target {
                break 'fill;
            }
        }
    }
    // Loose files in the cabinet root.
    write(&root.join("README old system.txt"), &mut files);
    // The cabinet is created over the tree: everything is indexed, nothing bound yet.
    vf_fs::rebuild::rebuild(&mut conn, &root, "dev").unwrap();
    Fx {
        _t: t,
        root,
        conn,
        files,
    }
}

/// Every file under `dir` (but `_Vibe`) → SHA-256.
fn snapshot(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for f in vf_fs::rebuild::cabinet_files(dir) {
        out.insert(
            f.strip_prefix(dir).unwrap().to_string_lossy().into_owned(),
            filer_intake::commit::sha256_file(&f).unwrap(),
        );
    }
    out
}

#[test]
fn a_filecenter_tree_of_2000_files_imports_mostly_mapped() {
    let mut f = tree(2000);
    assert!(f.files >= 2000);
    let before = snapshot(&f.root);
    let plan = import::plan(&f.conn, &f.root).unwrap();
    assert_eq!(plan.files.len(), f.files);
    let mapped = plan.auto_mapped();
    let share = mapped as f64 / f.files as f64;
    eprintln!(
        "auto-mapped {mapped} of {} ({:.1}%)",
        f.files,
        share * 100.0
    );
    assert!(share >= 0.90, "auto-mapped {:.1}%", share * 100.0);

    // What was mapped is right: every mapped file has the doc type its name or folder says.
    for r in &plan.files {
        if plan.gaps(r).is_empty() {
            let file = r.rel.rsplit('/').next().unwrap();
            assert_eq!(
                r.doc_type.as_ref().map(|d| d.1.as_str()),
                expected(file),
                "{}",
                r.rel
            );
        }
    }

    // The remainder is listed, with what each file lacks.
    let rest = plan.remainder();
    assert_eq!(rest.len(), f.files - mapped);
    assert!(
        rest.iter()
            .any(|(r, g)| r.rel.contains("scan0001") && g.contains(&Gap::DocType))
    );
    assert!(
        rest.iter()
            .any(|(r, g)| r.rel.starts_with("Scans to sort/") && g.contains(&Gap::Client))
    );
    let unmapped: Vec<&str> = plan
        .unmapped_folders()
        .iter()
        .map(|r| r.rel.as_str())
        .collect();
    assert_eq!(unmapped, ["Old Clients - Misc", "Scans to sort"]);
    let csv = plan.report_csv();
    assert!(csv.starts_with("kind,path,missing"));
    assert_eq!(csv.lines().count(), 1 + unmapped.len() + rest.len());

    // Apply: bound in place, nothing moved, metadata set.
    let applied = import::apply(&mut f.conn, &f.root, &plan, &Options::default(), "dev").unwrap();
    assert_eq!(applied.bound, plan.folders.len() - 2);
    assert_eq!(applied.mapped, mapped);
    assert_eq!(
        snapshot(&f.root),
        before,
        "bind mode moves and changes nothing"
    );
    let with_client: i64 = f
        .conn
        .query_row(
            "SELECT count(*) FROM document WHERE client_id IS NOT NULL AND deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let in_unmapped = plan
        .files
        .iter()
        .filter(|r| r.folder.is_none_or(|i| !plan.folders[i].include))
        .count();
    assert_eq!(with_client as usize, f.files - in_unmapped);
    let typed: i64 = f
        .conn
        .query_row(
            "SELECT count(*) FROM document WHERE doc_type_id IS NOT NULL AND client_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(typed as usize >= mapped);
    // A W-2 in a `TY2022` folder got its type and year.
    let (t, y): (String, Option<i64>) = f
        .conn
        .query_row(
            "SELECT t.name, d.tax_year FROM document d JOIN doc_type t ON t.id = d.doc_type_id
             WHERE d.rel_path LIKE '%/TY%/%W2 Acme Corp.pdf' LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(t, "W-2");
    assert!(y.is_some());

    // A rebuild keeps what the import set (docmeta), and a second plan has nothing new to bind.
    vf_fs::rebuild::rebuild(&mut f.conn, &f.root, "dev").unwrap();
    let typed2: i64 = f
        .conn
        .query_row(
            "SELECT count(*) FROM document WHERE doc_type_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(typed2, typed);
    let again = import::plan(&f.conn, &f.root).unwrap();
    assert!(again.folders.iter().all(|r| r.bound || !r.include));

    // Undo: sentinels gone, documents detached, every file where and as it was.
    import::undo(&mut f.conn, &f.root, &applied, "dev").unwrap();
    assert_eq!(snapshot(&f.root), before);
    for r in &plan.folders {
        assert!(!f.root.join(&r.rel).join("_Vibe/client.json").exists());
    }
    let with_client: i64 = f
        .conn
        .query_row(
            "SELECT count(*) FROM document WHERE client_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(with_client, 0);
}

#[test]
fn rename_to_the_template_moves_folders_and_undo_puts_them_back() {
    let mut f = tree(60);
    let before = snapshot(&f.root);
    let plan = import::plan(&f.conn, &f.root).unwrap();
    let renames: Vec<(String, String)> = plan
        .folders
        .iter()
        .filter(|r| r.include)
        .filter_map(|r| Some((r.rel.clone(), r.template_name.clone()?)))
        .collect();
    assert!(!renames.is_empty());
    let applied = import::apply(
        &mut f.conn,
        &f.root,
        &plan,
        &Options { rename: true },
        "dev",
    )
    .unwrap();
    assert_eq!(applied.renamed, renames.len());
    for (old, new) in &renames {
        assert!(!f.root.join(old).exists());
        assert!(f.root.join(new).join("_Vibe/client.json").is_file());
    }
    // Documents followed their folders and kept their metadata.
    let typed: i64 = f
        .conn
        .query_row(
            "SELECT count(*) FROM document WHERE doc_type_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(typed > 0);
    import::undo(&mut f.conn, &f.root, &applied, "dev").unwrap();
    assert_eq!(snapshot(&f.root), before);
}

/// QA 2026-11-03: a Filer Desktop tree with sentinels but no client list imported yet. Its
/// folders are bound already (as the rebuild treats them); the import types their files.
#[test]
fn sentinel_folders_without_a_client_list_count_as_bound() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("Clients");
    vf_fixtures::generate(&root, 3).unwrap();
    let mut conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
    vf_fs::doctypes::seed_if_missing(&root).unwrap();
    vf_fs::doctypes::sync(&mut conn, &root, "dev", "seed").unwrap();
    vf_fs::rebuild::rebuild(&mut conn, &root, "dev").unwrap();
    let plan = import::plan(&conn, &root).unwrap();
    assert_eq!(plan.folders.len(), 5);
    assert!(
        plan.folders.iter().all(|r| r.bound && r.client.is_some()),
        "{:?}",
        plan.folders
    );
    assert!(
        plan.files
            .iter()
            .all(|f| !plan.gaps(f).contains(&Gap::Client))
    );
    let applied = import::apply(&mut conn, &root, &plan, &Options::default(), "dev").unwrap();
    assert_eq!(applied.bound, 0, "nothing to bind");
    assert!(applied.mapped > 0, "files got their doc types and years");
}

/// QA P4/P5/P14: folders that only look like a client are not bound on their own.
#[test]
fn look_alike_folders_wait_for_a_person() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("Clients");
    std::fs::create_dir_all(&root).unwrap();
    vf_fs::cabinet::create(
        &root,
        &CabinetFile {
            cabinet_id: vf_core::new_id(),
            firm_id: uuid::Uuid::new_v4().to_string(),
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
    std::fs::write(
        &csv,
        "ID,Name\nSMIT1001,\"Smith, John\"\nANDE1042,Anderson Farms LLC\nSMIT2002,Smith\nREYE3003,\"Reyes, Dana\"\n",
    )
    .unwrap();
    let map = filer_core::config::ColumnMap {
        id: Some("ID".into()),
        name: Some("Name".into()),
        extras: vec![],
    };
    vf_fs::clients::import_csv(&root, &csv, &map, &Default::default()).unwrap();
    for f in [
        "Smith LLC",               // one meaningful word against "Smith, John"
        "Smith Family Trust",      // more words than the client "Smith"
        "Baker Dental (ANDE1042)", // Anderson's ID, another name
        "Tax Returns (2019)",      // a year, not an ID
        "Reyes (DECEASED)",        // a word, not an ID
        "_Archive",                // skipped by name
    ] {
        let p = root.join(f).join("2023").join("a.pdf");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, f).unwrap();
    }
    // A genuine one binds.
    let p = root.join("John Smith").join("2023").join("W2 Acme.pdf");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "w2").unwrap();
    vf_fs::rebuild::rebuild(&mut conn, &root, "dev").unwrap();
    let plan = import::plan(&conn, &root).unwrap();
    let get = |n: &str| plan.folders.iter().find(|r| r.rel == n).unwrap();
    for f in [
        "Smith LLC",
        "Smith Family Trust",
        "Baker Dental (ANDE1042)",
        "Tax Returns (2019)",
        "Reyes (DECEASED)",
    ] {
        let r = get(f);
        assert!(!r.include, "{f}: {r:?}");
        assert!(
            r.client.as_ref().is_none_or(|c| !c.new),
            "{f} became a new client"
        );
    }
    assert!(get("John Smith").include, "{:?}", get("John Smith"));
    assert_eq!(plan.skipped, ["_Archive"]);
    assert!(plan.report_csv().contains("_Archive"));
}

/// QA P12: a year from a file name or a doc type from a folder name is a guess: tagged, kept
/// out of disposition until a person sets it.
#[test]
fn import_guesses_wait_for_a_person_before_disposition() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("Clients");
    std::fs::create_dir_all(&root).unwrap();
    vf_fs::cabinet::create(
        &root,
        &CabinetFile {
            cabinet_id: vf_core::new_id(),
            firm_id: uuid::Uuid::new_v4().to_string(),
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
    vf_fs::retention::seed_if_missing(&root).unwrap();
    vf_fs::retention::sync(&mut conn, &root, "dev", "seed").unwrap();
    let csv = t.path().join("clients.csv");
    std::fs::write(&csv, "ID,Name\nSMIT1001,\"Smith, John\"\n").unwrap();
    let map = filer_core::config::ColumnMap {
        id: Some("ID".into()),
        name: Some("Name".into()),
        extras: vec![],
    };
    vf_fs::clients::import_csv(&root, &csv, &map, &Default::default()).unwrap();
    for (p, body) in [
        ("John Smith (SMIT1001)/Old Papers/W2 1999 Oak St.pdf", "w2 1999"),
        ("John Smith (SMIT1001)/Bank/2010/closing.pdf", "closing"),
    ] {
        let p = root.join(p);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    vf_fs::rebuild::rebuild(&mut conn, &root, "dev").unwrap();
    let plan = import::plan(&conn, &root).unwrap();
    let applied = import::apply(&mut conn, &root, &plan, &Options::default(), "dev").unwrap();
    assert!(applied.guessed >= 1, "{applied:?}");
    let tagged: Vec<String> = {
        let mut st = conn
            .prepare(
                "SELECT d.rel_path FROM document_tag t JOIN document d ON d.id = t.doc_id
                 WHERE t.tag = ?1",
            )
            .unwrap();
        st.query_map([vf_fs::docmeta::GUESSED_TAG], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert!(!tagged.is_empty());
    let today = time::Date::from_calendar_date(2040, time::Month::January, 1).unwrap();
    let rep = vf_fs::disposition::eligible(&conn, &root, today).unwrap();
    assert!(
        rep.lines.iter().all(|l| !tagged.contains(&l.rel_path)),
        "guessed documents are not offered"
    );
    assert_eq!(rep.guessed, tagged.len());
    // A person sets the year: the tag goes, the document can come up.
    let id: String = conn
        .query_row(
            "SELECT id FROM document WHERE rel_path = ?1",
            [&tagged[0]],
            |r| r.get(0),
        )
        .unwrap();
    let edit = vf_fs::docmeta::MetaEdit {
        tax_year: Some(Some(2001)),
        ..Default::default()
    };
    vf_fs::docmeta::edit_each(&mut conn, &root, &[(id.clone(), edit)], "meta_edit", "dev").unwrap();
    assert!(
        !vf_fs::docmeta::load(&conn, &id)
            .unwrap()
            .tags
            .iter()
            .any(|t| t == vf_fs::docmeta::GUESSED_TAG)
    );
}
