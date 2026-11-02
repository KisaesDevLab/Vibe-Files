//! Print capture (PHASES P02 step 5). PDFs a PDF printer saves into the print-capture folder
//! enter Intake as firm documents. A parser profile (`<data>/config/parsers/*.toml`, editable
//! regexes, QUESTIONS Q-09) reads the client ID, tax year and form from the tax software's file
//! name; a match is renamed the Filer way with a doc type, like a cover-sheet piece
//! (`crate::stack`), so it files with no edits. Anything else is queued as it is.

use std::path::{Path, PathBuf};

use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use vf_core::db::Connection;

use crate::incoming::{Pass, queue_as_is, ready_files};
use crate::intake::{Hint, Hints};
use crate::stack::MARK;

/// Profiles written on first use (never over a file the firm edited).
pub const DEFAULTS: [(&str, &str); 5] = [
    (
        "ultratax.toml",
        include_str!("../assets/parsers/ultratax.toml"),
    ),
    (
        "lacerte.toml",
        include_str!("../assets/parsers/lacerte.toml"),
    ),
    (
        "cch-axcess.toml",
        include_str!("../assets/parsers/cch-axcess.toml"),
    ),
    ("drake.toml", include_str!("../assets/parsers/drake.toml")),
    (
        "proseries.toml",
        include_str!("../assets/parsers/proseries.toml"),
    ),
];

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DocTypeMap {
    #[serde(rename = "match")]
    pub pattern: String,
    pub doc_type: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    /// `primary`, `secondary` or `untested`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub order: i64,
    pub pattern: String,
    #[serde(default)]
    pub default_doc_type: Option<String>,
    #[serde(default)]
    pub doc_types: Vec<DocTypeMap>,
}

/// A profile with its regexes compiled.
#[derive(Debug, Clone)]
pub struct Compiled {
    pub file: PathBuf,
    pub profile: Profile,
    pattern: Regex,
    doc_types: Vec<(Regex, String)>,
}

pub fn parsers_dir(data: &Path) -> PathBuf {
    data.join("config").join("parsers")
}

/// Writes the default profiles that are missing.
pub fn seed_profiles(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    for (name, text) in DEFAULTS {
        let p = dir.join(name);
        if !p.exists() {
            std::fs::write(&p, text).map_err(|e| format!("cannot write {}: {e}", p.display()))?;
        }
    }
    Ok(())
}

/// Every `*.toml` profile in `dir`, compiled, in `order`; unreadable ones are reported.
pub fn load_profiles(dir: &Path) -> (Vec<Compiled>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|e| e == "toml"));
    files.sort();
    for f in files {
        let r = std::fs::read_to_string(&f)
            .map_err(|e| e.to_string())
            .and_then(|t| toml::from_str::<Profile>(&t).map_err(|e| e.to_string()))
            .and_then(|profile| {
                let pattern = RegexBuilder::new(&profile.pattern)
                    .case_insensitive(true)
                    .build()
                    .map_err(|e| format!("pattern: {e}"))?;
                if !pattern.capture_names().any(|n| n == Some("id")) {
                    return Err("pattern needs a named group `id`".into());
                }
                let doc_types = profile
                    .doc_types
                    .iter()
                    .map(|m| Regex::new(&m.pattern).map(|r| (r, m.doc_type.clone())))
                    .collect::<Result<_, _>>()
                    .map_err(|e| format!("doc_types: {e}"))?;
                Ok(Compiled {
                    file: f.clone(),
                    profile,
                    pattern,
                    doc_types,
                })
            });
        match r {
            Ok(c) => out.push(c),
            Err(e) => errors.push(format!("{}: {e}", f.display())),
        }
    }
    out.sort_by(|a, b| (a.profile.order, &a.profile.name).cmp(&(b.profile.order, &b.profile.name)));
    (out, errors)
}

/// What a profile read from a file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub profile: String,
    pub ext_id: String,
    pub tax_year: Option<i64>,
    pub doc_type: Option<String>,
}

/// The first profile whose pattern matches `stem` (file name without extension).
pub fn parse(profiles: &[Compiled], stem: &str) -> Option<Parsed> {
    profiles.iter().find_map(|c| {
        let caps = c.pattern.captures(stem)?;
        let ext_id = caps.name("id")?.as_str().trim().to_owned();
        let tax_year = caps.name("year").and_then(|y| y.as_str().parse().ok());
        let doc_type = c
            .doc_types
            .iter()
            .find(|(r, _)| r.is_match(stem))
            .map(|(_, d)| d.clone())
            .or_else(|| c.profile.default_doc_type.clone());
        Some(Parsed {
            profile: c.profile.name.clone(),
            ext_id,
            tax_year,
            doc_type,
        })
    })
}

fn client_by_ext(conn: &Connection, ext: &str) -> Option<String> {
    conn.query_row(
        "SELECT name FROM client WHERE ext_id = ?1 COLLATE NOCASE AND deleted_at IS NULL",
        [ext],
        |r| r.get(0),
    )
    .ok()
}

/// One pass over the print-capture folder.
pub fn process_capture(
    conn: &Connection,
    capture: &Path,
    inbox: &Path,
    parsers: &Path,
    hints: &Hints,
) -> Pass {
    let mut pass = Pass::default();
    let files = ready_files(capture, &["pdf"]);
    if files.is_empty() {
        return pass;
    }
    let (profiles, errors) = load_profiles(parsers);
    pass.errors.extend(errors);
    for f in files {
        let stem = f
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let parsed = parse(&profiles, &stem);
        let named = parsed
            .as_ref()
            .and_then(|p| client_by_ext(conn, &p.ext_id).map(|n| (p, n)));
        let matched = named.is_some();
        let (dst, hint) = match named {
            Some((p, client)) => {
                let mut rest = String::new();
                if let Some(y) = p.tax_year {
                    rest.push_str(&format!("{y} "));
                }
                rest.push_str(p.doc_type.as_deref().unwrap_or("Document"));
                let name = format!(
                    "{}_{}_{} {MARK}.pdf",
                    filer_core::template::sanitize_value(&client),
                    filer_core::template::sanitize_value(&p.ext_id),
                    filer_core::template::sanitize_value(&rest),
                );
                let _ = std::fs::create_dir_all(inbox);
                let dst = filer_intake::commit::free_path(&inbox.join(name));
                let moved = std::fs::rename(&f, &dst)
                    .map_err(|e| format!("cannot move {} to the inbox: {e}", f.display()));
                (
                    moved.map(|()| dst),
                    Hint {
                        doc_type: p.doc_type.clone(),
                        source: Some("firm".into()),
                        tax_year: p.tax_year,
                    },
                )
            }
            None => (
                queue_as_is(&f, inbox),
                Hint {
                    source: Some("firm".into()),
                    ..Hint::default()
                },
            ),
        };
        match dst {
            Ok(d) => {
                if let Ok(mut h) = hints.lock() {
                    h.insert(d, hint);
                }
                pass.routed += usize::from(matched);
                pass.queued += usize::from(!matched);
            }
            Err(e) => pass.errors.push(e),
        }
    }
    pass
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles() -> Vec<Compiled> {
        let tmp = tempfile::tempdir().unwrap();
        seed_profiles(tmp.path()).unwrap();
        let (p, errors) = load_profiles(tmp.path());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(p.len(), 5);
        assert_eq!(p[0].profile.name, "UltraTax CS");
        p
    }

    #[test]
    fn default_profiles_parse_their_conventions() {
        let p = profiles();
        let got = |s: &str| parse(&p, s).map(|x| (x.profile, x.ext_id, x.tax_year, x.doc_type));
        assert_eq!(
            got("1042_2025_Federal Return"),
            Some((
                "UltraTax CS".into(),
                "1042".into(),
                Some(2025),
                Some("Return copy — federal".into())
            ))
        );
        assert_eq!(
            got("1042 2025 IL-1040 State Return").unwrap().3.as_deref(),
            Some("Return copy — state")
        );
        assert_eq!(
            got("1042-2025-8879").unwrap().3.as_deref(),
            Some("Form 8879")
        );
        assert_eq!(got("Invoice March"), None);
    }

    #[test]
    fn edited_profiles_are_never_overwritten_and_bad_ones_reported() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("ultratax.toml"),
            "name = \"Mine\"\npattern = '^(?P<id>X\\d+)$'\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("broken.toml"),
            "name = \"Broken\"\npattern = '^(no id)$'\n",
        )
        .unwrap();
        seed_profiles(tmp.path()).unwrap();
        let (p, errors) = load_profiles(tmp.path());
        assert!(p.iter().any(|c| c.profile.name == "Mine"));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("named group `id`"));
    }

    /// P02 done-when: a PDF the system PDF printer saved into the capture folder with an
    /// UltraTax-style name files to the client's year folder with doc type "Return copy —
    /// federal". (The template's folder for that doc type is `Income Tax/{Year}`; UI_SPEC's
    /// `<Client>/<Year>/Returns/` is illustrative, CLAUDE.md §Layout.)
    #[test]
    fn ultratax_print_capture_files_as_federal_return_copy() {
        use crate::clients::{NewClient, create_client, template as cab_template};
        use crate::intake::prepare_filer_dir;
        use filer_intake::plan::RowStatus;

        let mut w = crate::clients::tests::world();
        vf_core::doctype::seed(&mut w.conn, &w.actor).unwrap();
        let tpl = cab_template(&w.root);
        let nc = NewClient {
            ext_id: "1042".into(),
            name: "Anderson Farms LLC".into(),
            ..NewClient::default()
        };
        create_client(&mut w.conn, &w.root, &w.cab, &tpl, &nc, &w.actor).unwrap();
        let base = w.root.parent().unwrap().to_path_buf();
        let (filer, inbox, capture, parsers) = (
            base.join("data/filer"),
            base.join("data/Inbox"),
            base.join("data/Print capture"),
            base.join("data/config/parsers"),
        );
        prepare_filer_dir(&mut w.conn, &w.actor, &filer, &w.root, &w.cab, &tpl, &inbox).unwrap();
        seed_profiles(&parsers).unwrap();

        // What "Microsoft Print to PDF" leaves: a PDF with the name typed in its Save dialog.
        let printed = capture.join("1042_2025_Federal Return.pdf");
        vf_fixtures::write_pdf(&printed, &["Form 1040 (synthetic)".into()]).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&printed)
            .unwrap()
            .set_modified(old)
            .unwrap();

        let hints = Hints::default();
        let pass = process_capture(&w.conn, &capture, &inbox, &parsers, &hints);
        assert_eq!(
            (pass.routed, pass.errors.len()),
            (1, 0),
            "{:?}",
            pass.errors
        );
        let queued = inbox.join("Anderson Farms LLC_1042_2025 Return copy — federal (vf).pdf");
        assert!(queued.is_file());

        let desk = crate::testkit::Desk {
            root: &w.root,
            filer: &filer,
            inbox: &inbox,
            tpl: &tpl,
            cab: &w.cab,
        };
        let out = desk.plan(std::slice::from_ref(&queued));
        assert_eq!(
            out.proposals[0].status,
            RowStatus::Ready,
            "{:?}",
            out.proposals[0].dest
        );
        let report = desk.commit(&out, &base.join("c.db"), &w.actor, &hints);
        assert!(
            report.outcomes.iter().all(|o| o.result.is_ok()),
            "{:?}",
            report.outcomes
        );

        let (rel, dt, src, yr): (String, String, String, i64) = w
            .conn
            .query_row(
                "SELECT d.rel_path, t.name, d.source, d.tax_year FROM document d JOIN doc_type t ON t.id = d.doc_type_id",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            rel,
            "Anderson Farms LLC (1042)/Income Tax/2025/2025 Return copy — federal - Anderson Farms LLC.pdf"
        );
        assert_eq!(
            (dt.as_str(), src.as_str(), yr),
            ("Return copy — federal", "firm", 2025)
        );
    }
}
