//! Test helpers: run Filer Desktop's own planner and commit over files in an inbox, the way the
//! Intake tab does, so tests check what a user would see in Preview.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use filer_core::rules;
use filer_core::template::TemplateNode;
use filer_intake::commit::{CommitInput, CommitReport, CommitTarget};
use filer_intake::intake::{Kind, QueuedFile, Source};
use filer_intake::plan::{self, PlanInput, PlanOutput, Rescan};
use filer_intake::sentinel::{self, FoldersFile, ScanInput};
use uuid::Uuid;

use crate::cabinet::CabinetFile;
use crate::intake::{CatalogTarget, Hints, hooks};

pub struct Desk<'a> {
    pub root: &'a Path,
    pub filer: &'a Path,
    pub inbox: &'a Path,
    pub tpl: &'a TemplateNode,
    pub cab: &'a CabinetFile,
}

impl Desk<'_> {
    pub fn plan(&self, files: &[PathBuf]) -> PlanOutput {
        let clients = Arc::new(
            filer_core::clients::load_cache(self.filer)
                .unwrap()
                .unwrap(),
        );
        let bindings = sentinel::scan_root(
            &ScanInput {
                root: self.root.to_path_buf(),
                clients: clients.clone(),
                id_pattern: filer_core::parse::DEFAULT_ID_PATTERN.into(),
                threshold: 0.85,
                ignored: vec![],
            },
            &Default::default(),
        )
        .unwrap();
        plan::build_plan(&PlanInput {
            files: files
                .iter()
                .map(|p| QueuedFile {
                    path: p.clone(),
                    size: std::fs::metadata(p).unwrap().len(),
                    added: chrono::Local::now(),
                    source: Source::Inbox(self.inbox.to_path_buf()),
                    kind: Kind::Pdf,
                })
                .collect(),
            edits: HashMap::new(),
            batch: Default::default(),
            clients: Some(clients),
            rules: rules::load_or_seed(self.filer).0.active_rules().to_vec(),
            template: self.tpl.clone(),
            folders: Some(FoldersFile {
                root: self.root.to_path_buf(),
                scanned_at: chrono::Utc::now(),
                folders: bindings,
                ignored: vec![],
            }),
            root: Some(self.root.to_path_buf()),
            id_pattern: filer_core::parse::DEFAULT_ID_PATTERN.into(),
            fuzzy_suggest: filer_core::config::DEFAULT_FUZZY_SUGGEST,
            fuzzy_auto: filer_core::config::DEFAULT_FUZZY_AUTO,
            current_year: 2026,
            sniff_enabled: false,
            sniff: HashMap::new(),
            rescan: Rescan::Done,
            partial: vec![],
            relationships: Arc::new(Default::default()),
        })
    }

    /// Commits the plan through the Vibe Files hooks (Filer's commit + the catalog).
    pub fn commit(
        &self,
        out: &PlanOutput,
        catalog: &Path,
        actor: &vf_core::tx::Actor,
        hints: &Hints,
    ) -> CommitReport {
        let input = CommitInput::from_plan(
            &out.proposals,
            &HashMap::new(),
            &[],
            CommitTarget {
                root: self.root.to_path_buf(),
                first_inbox: Some(self.inbox.to_path_buf()),
                data_dir: self.filer.to_path_buf(),
                firm_id: Uuid::parse_str(&self.cab.firm_id).unwrap(),
                template: self.tpl.clone(),
            },
        );
        let target = CatalogTarget {
            catalog: catalog.to_path_buf(),
            key: vf_core::db::CatalogKey([5; 32]),
            root: self.root.to_path_buf(),
            actor: actor.clone(),
            hints: hints.clone(),
        };
        (hooks(target).commit)(&input, &AtomicUsize::new(0))
    }
}
