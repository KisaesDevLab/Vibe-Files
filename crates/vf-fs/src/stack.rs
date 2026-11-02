//! Scanned stacks with barcode cover sheets → documents in the Intake queue (PHASES P02 step 3).
//!
//! [`route_scan`] finds the sheets in a scanned PDF (`vf_ingest::cover`), splits it at them with
//! Filer's `split_pdf`, and puts one PDF per document in the Filer inbox, named the Filer way:
//! `{Client}_{ID}_{Year} {DocType} (vf).pdf`. Each gets a [`Hint`] (doc type, year, source) for
//! the catalog. [`sync_doctype_rules`] keeps one Filer rule per doc type that matches exactly
//! those names and routes them to the doc type's default folder, so a covered document comes up
//! Ready in Preview with no edits.

use std::path::{Path, PathBuf};

use filer_core::rules::{self, Rule};
use filer_core::template;
use pdfium_render::prelude::Pdfium;
use vf_core::db::Connection;
use vf_ingest::cover::{self, Segment};

use crate::intake::{Hint, Hints};

/// Generated Filer rules carry this id prefix; they are rewritten on every sync.
pub const RULE_PREFIX: &str = "vf-dt-";
/// The `{Rest}` suffix that marks a file Vibe Files prepared.
pub const MARK: &str = "(vf)";

fn regex_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// One Filer rule per live doc type with a default folder: matches `[YYYY ]<name> (vf)` and
/// files to `{Client}/<default folder>`.
pub fn doctype_rules(conn: &Connection) -> vf_core::Result<Vec<Rule>> {
    let types = vf_core::doctype::list(conn, false)?;
    Ok(types
        .into_iter()
        .filter_map(|t| {
            let folder = t.default_folder_tpl.filter(|f| !f.trim().is_empty())?;
            let requires_year = folder.contains("{Year}");
            let filename = if requires_year {
                "{Year} {DocType} - {Client}.{Ext}"
            } else {
                "{DocType} - {Client}.{Ext}"
            };
            let mut r = Rule::new(
                &t.name,
                &format!(r"^(?:\d{{4}} )?{} \(vf\)$", regex_escape(&t.name)),
                &format!("{{Client}}/{folder}"),
                filename,
                requires_year,
            );
            r.id = format!("{RULE_PREFIX}{}", t.id);
            Some(r)
        })
        .collect())
}

/// Rewrites the generated rules at the top of Filer's active profile (keeping each one's
/// enabled switch); the firm's own rules are untouched.
pub fn sync_doctype_rules(conn: &Connection, filer_dir: &Path) -> Result<(), String> {
    let (mut file, savable, err) = rules::load_or_seed(filer_dir);
    if !savable {
        return Err(err.unwrap_or_else(|| "rules.json is not readable".into()));
    }
    let generated = doctype_rules(conn).map_err(|e| e.to_string())?;
    let active = file.active_rules_mut();
    let enabled: std::collections::HashMap<String, bool> = active
        .iter()
        .filter(|r| r.id.starts_with(RULE_PREFIX))
        .map(|r| (r.id.clone(), r.enabled))
        .collect();
    active.retain(|r| !r.id.starts_with(RULE_PREFIX));
    for (i, mut r) in generated.into_iter().enumerate() {
        r.enabled = enabled.get(&r.id).copied().unwrap_or(true);
        active.insert(i, r);
    }
    file.normalize();
    rules::save(filer_dir, &file).map_err(|e| e.to_string())
}

/// A document of a split stack, placed in the inbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routed {
    pub path: PathBuf,
    pub hint: Hint,
    /// Catalog client id, when the cover named one.
    pub client_id: Option<String>,
}

fn client_of(conn: &Connection, id: &str) -> Option<(String, String)> {
    conn.query_row(
        "SELECT name, ext_id FROM client WHERE id = ?1 AND deleted_at IS NULL",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

fn free(dir: &Path, name: &str) -> PathBuf {
    filer_intake::commit::free_path(&dir.join(name))
}

/// Splits `stack` at its cover / separator sheets into the inbox. `Ok(None)` when it has no
/// sheets (the caller queues it as it is). The stack itself is moved to `processed`, never
/// deleted.
pub fn route_scan(
    conn: &Connection,
    pdfium: &Pdfium,
    stack: &Path,
    inbox: &Path,
    processed: &Path,
    hints: &Hints,
) -> Result<Option<Vec<Routed>>, String> {
    let pages = cover::scan(pdfium, stack)?;
    if pages.iter().all(Option::is_none) {
        return Ok(None);
    }
    let segs: Vec<Segment> = cover::segments(&pages);
    let stem = stack
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "scan".into());
    // Pieces are written beside `processed`, then renamed into the inbox whole.
    let work = processed.join(format!("~work-{}", vf_core::new_id()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    let mut routed = Vec::new();
    for (n, seg) in segs.iter().enumerate() {
        let p = seg.payload.clone().unwrap_or_default();
        let client = (!p.client_id.is_empty())
            .then(|| client_of(conn, &p.client_id))
            .flatten();
        let name = match &client {
            Some((cname, ext)) => {
                let mut rest = String::new();
                if let Some(y) = p.tax_year {
                    rest.push_str(&format!("{y} "));
                }
                rest.push_str(if p.doc_type.is_empty() {
                    "Document"
                } else {
                    &p.doc_type
                });
                format!(
                    "{}_{}_{} {MARK}.pdf",
                    template::sanitize_value(cname),
                    template::sanitize_value(ext),
                    template::sanitize_value(&rest)
                )
            }
            None => format!("{} part {}.pdf", template::sanitize_value(&stem), n + 1),
        };
        names.push(work.join(&name));
        routed.push(Routed {
            path: PathBuf::from(name),
            hint: Hint {
                doc_type: (!p.doc_type.is_empty()).then(|| p.doc_type.clone()),
                source: Some("client".into()),
                tax_year: p.tax_year,
            },
            client_id: client.map(|_| p.client_id.clone()),
        });
    }
    let split = cover::split_stack(stack, &segs, &names);
    if let Err(e) = split {
        let _ = std::fs::remove_dir(&work);
        return Err(e);
    }
    std::fs::create_dir_all(inbox).map_err(|e| e.to_string())?;
    let mut map = hints
        .lock()
        .map_err(|_| "hint table is poisoned".to_string())?;
    for (r, piece) in routed.iter_mut().zip(&names) {
        let dst = free(inbox, &r.path.to_string_lossy());
        std::fs::rename(piece, &dst)
            .map_err(|e| format!("cannot move {} into the inbox: {e}", piece.display()))?;
        map.insert(dst.clone(), r.hint.clone());
        r.path = dst;
    }
    drop(map);
    let _ = std::fs::remove_dir(&work);
    let kept = free(
        processed,
        &stack.file_name().unwrap_or_default().to_string_lossy(),
    );
    std::fs::rename(stack, &kept)
        .map_err(|e| format!("cannot move {} aside: {e}", stack.display()))?;
    Ok(Some(routed))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use filer_core::clients as fclients;
    use filer_intake::commit::{CommitInput, CommitTarget};
    use filer_intake::intake::{Kind, QueuedFile, Source};
    use filer_intake::plan::{self, PlanInput, Rescan, RowStatus};
    use filer_intake::sentinel::{self, FoldersFile, ScanInput};
    use uuid::Uuid;
    use vf_ingest::cover::Sheet;

    use super::*;
    use crate::clients::{NewClient, create_client, template as cab_template};
    use crate::intake::{CatalogTarget, hooks, prepare_filer_dir};

    fn pdfium() -> &'static Pdfium {
        let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium.dll");
        filer_pdf::pdf::pdfium(&dir).unwrap()
    }

    /// P02 done-when: a 40-page scanned stack with 4 cover sheets splits into 4 documents filed
    /// to the right clients with no manual edits.
    #[test]
    fn forty_page_stack_with_four_covers_files_four_documents() {
        let mut w = crate::clients::tests::world();
        vf_core::doctype::seed(&mut w.conn, &w.actor).unwrap();
        let tpl = cab_template(&w.root);
        let people = [
            ("1042", "Anderson Farms LLC"),
            ("1150", "Delgado, Maria"),
            ("1001", "Baker, Ann"),
            ("1002", "Cole Co"),
        ];
        let ids: Vec<String> = people
            .iter()
            .map(|(e, n)| {
                let nc = NewClient {
                    ext_id: (*e).into(),
                    name: (*n).into(),
                    ..NewClient::default()
                };
                create_client(&mut w.conn, &w.root, &w.cab, &tpl, &nc, &w.actor)
                    .unwrap()
                    .0
            })
            .collect();
        let base = w.root.parent().unwrap().to_path_buf();
        let (filer, inbox) = (base.join("data/filer"), base.join("data/Inbox"));
        prepare_filer_dir(&mut w.conn, &w.actor, &filer, &w.root, &w.cab, &tpl, &inbox).unwrap();

        // The stack: cover + 9 pages, four times (W-2 2025, K-1 2024, EIN letter, Organizer 2025).
        let docs = [
            ("W-2", Some(2025)),
            ("K-1 (1065)", Some(2024)),
            ("EIN letter", None),
            ("Organizer", Some(2025)),
        ];
        let parts = base.join("parts");
        let mut inputs = Vec::new();
        for (i, ((e, n), (dt, y))) in people.iter().zip(docs).enumerate() {
            let c = parts.join(format!("{i}-cover.pdf"));
            cover::write_sheets(&c, &[Sheet::cover(&ids[i], n, e, dt, y)]).unwrap();
            inputs.push((c, String::new()));
            for pg in 0..9 {
                let p = parts.join(format!("{i}-{pg}.pdf"));
                vf_fixtures::write_pdf(&p, &[format!("{n} {dt} page {}", pg + 1)]).unwrap();
                inputs.push((p, String::new()));
            }
        }
        let scans = base.join("data/Scans");
        std::fs::create_dir_all(&scans).unwrap();
        let stack = scans.join("stack.pdf");
        assert_eq!(filer_pdf::pdf::merge_pdfs(&inputs, &stack).unwrap(), 40);

        let hints = Hints::default();
        let processed = base.join("data/Scans/processed");
        let routed = route_scan(&w.conn, pdfium(), &stack, &inbox, &processed, &hints)
            .unwrap()
            .unwrap();
        assert_eq!(routed.len(), 4);
        assert!(
            !stack.exists() && processed.join("stack.pdf").is_file(),
            "original kept aside"
        );
        for r in &routed {
            assert_eq!(filer_pdf::pdf::page_count(&r.path).unwrap(), 9);
        }

        // Filer's own planner, with its rules (generated per doc type) and folder index.
        let clients = Arc::new(fclients::load_cache(&filer).unwrap().unwrap());
        let (rules_file, _, _) = rules::load_or_seed(&filer);
        let bindings = sentinel::scan_root(
            &ScanInput {
                root: w.root.clone(),
                clients: clients.clone(),
                id_pattern: filer_core::parse::DEFAULT_ID_PATTERN.into(),
                threshold: 0.85,
                ignored: vec![],
            },
            &Default::default(),
        )
        .unwrap();
        let input = PlanInput {
            files: routed
                .iter()
                .map(|r| QueuedFile {
                    path: r.path.clone(),
                    size: std::fs::metadata(&r.path).unwrap().len(),
                    added: chrono::Local::now(),
                    source: Source::Inbox(inbox.clone()),
                    kind: Kind::Pdf,
                })
                .collect(),
            edits: HashMap::new(),
            batch: Default::default(),
            clients: Some(clients),
            rules: rules_file.active_rules().to_vec(),
            template: tpl.clone(),
            folders: Some(FoldersFile {
                root: w.root.clone(),
                scanned_at: chrono::Utc::now(),
                folders: bindings,
                ignored: vec![],
            }),
            root: Some(w.root.clone()),
            id_pattern: filer_core::parse::DEFAULT_ID_PATTERN.into(),
            fuzzy_suggest: filer_core::config::DEFAULT_FUZZY_SUGGEST,
            fuzzy_auto: filer_core::config::DEFAULT_FUZZY_AUTO,
            current_year: 2026,
            sniff_enabled: false,
            sniff: HashMap::new(),
            rescan: Rescan::Done,
            partial: vec![],
            relationships: Arc::new(Default::default()),
        };
        let out = plan::build_plan(&input);
        for p in &out.proposals {
            assert_eq!(
                p.status,
                RowStatus::Ready,
                "{} → {:?}",
                p.file.path.display(),
                p.dest
            );
        }

        let target = CatalogTarget {
            catalog: base.join("c.db"),
            key: vf_core::db::CatalogKey([5; 32]),
            root: w.root.clone(),
            actor: w.actor.clone(),
            hints: hints.clone(),
        };
        let commit_input = CommitInput::from_plan(
            &out.proposals,
            &HashMap::new(),
            &[],
            CommitTarget {
                root: w.root.clone(),
                first_inbox: Some(inbox.clone()),
                data_dir: filer.clone(),
                firm_id: Uuid::parse_str(&w.cab.firm_id).unwrap(),
                template: tpl.clone(),
            },
        );
        let report = (hooks(target).commit)(&commit_input, &AtomicUsize::new(0));
        assert!(
            report.outcomes.iter().all(|o| o.result.is_ok()),
            "{:?}",
            report.outcomes
        );

        let mut st = w
            .conn
            .prepare(
                "SELECT c.ext_id, t.name, d.tax_year, d.rel_path FROM document d
                 JOIN client c ON c.id = d.client_id JOIN doc_type t ON t.id = d.doc_type_id
                 WHERE d.deleted_at IS NULL ORDER BY c.ext_id",
            )
            .unwrap();
        let got: Vec<(String, String, Option<i64>, String)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(got.len(), 4, "{got:?}");
        let by = |e: &str| got.iter().find(|g| g.0 == e).unwrap().clone();
        assert_eq!(
            by("1042").3,
            "Anderson Farms LLC (1042)/Income Tax/2025/2025 W-2 - Anderson Farms LLC.pdf"
        );
        assert_eq!(
            (by("1150").1.as_str(), by("1150").2),
            ("K-1 (1065)", Some(2024))
        );
        assert_eq!(
            by("1001").3,
            "Baker, Ann (1001)/Permanent/Setup/EIN letter - Baker, Ann.pdf"
        );
        assert_eq!(by("1002").1, "Organizer");
    }
}
