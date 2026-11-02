//! Intake tab (PHASES P02 step 1): the vendored Filer Desktop engine, mounted as-is. Vibe Files
//! feeds it the cabinet (root, firm id, template, clients with catalog ids, relationships) and
//! sets its hooks so every batch is recorded in the catalog (`vf_fs::intake`).

use std::path::{Path, PathBuf};

use filer_core::config::{ColumnMap, Settings};
use filer_core::template::TemplateNode;
use filer_intake::app::{AppState, FilerApp};
use vf_fs::intake::{CatalogTarget, Hints};

use crate::session::{CATALOG_FILE, Session};

/// Filer's own data folder (config, rules, queue state, ops.jsonl, last_batch.json).
pub fn filer_dir(data: &Path) -> PathBuf {
    data.join("filer")
}

/// Watched by default when no inbox is set yet.
pub fn default_inbox(data: &Path) -> PathBuf {
    data.join("Inbox")
}

pub struct IntakeTab {
    pub app: FilerApp,
    pub dir: PathBuf,
    /// Client and relationship rows the Filer data folder was last written from.
    synced: String,
}

/// Changes when any client or relationship row changes.
fn client_signature(s: &Session) -> String {
    s.conn
        .query_row(
            "SELECT (SELECT count(*) || '/' || coalesce(max(updated_at), '') || '/' || coalesce(sum(rev), 0) FROM client)
                 || '|' || (SELECT count(*) || '/' || coalesce(max(updated_at), '') || '/' || coalesce(sum(rev), 0) FROM client_relationship)",
            [],
            |r| r.get(0),
        )
        .unwrap_or_default()
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

impl IntakeTab {
    pub fn open(
        ctx: &egui::Context,
        s: &mut Session,
        template: &TemplateNode,
        hints: Hints,
    ) -> Result<Self, String> {
        let dir = filer_dir(&s.data);
        vf_fs::intake::prepare_filer_dir(
            &mut s.conn,
            &s.actor,
            &dir,
            &s.root,
            &s.cabinet,
            template,
            &default_inbox(&s.data),
        )?;
        let (settings, savable) = Settings::load_or_default(&dir);
        let pdfium = filer_pdf::pdf::find_pdfium(&exe_dir());
        let mut state = AppState::load(dir.clone(), settings, savable, pdfium);
        state.hooks = vf_fs::intake::hooks(CatalogTarget {
            catalog: s.data.join(CATALOG_FILE),
            key: s.key.clone(),
            root: s.root.clone(),
            actor: s.actor.clone(),
            hints,
        });
        let app = FilerApp::new(ctx, state);
        // FilerApp installs Filer's theme; keep Vibe Files' look on the other tabs.
        crate::ui::theme::install(ctx);
        Ok(Self {
            app,
            dir,
            synced: client_signature(s),
        })
    }

    pub fn queue_len(&self) -> usize {
        self.app.queue_len()
    }

    /// Re-writes Filer's client list, template and relationships when the catalog's changed
    /// (not while a batch is filing).
    pub fn sync(
        &mut self,
        ctx: &egui::Context,
        s: &mut Session,
        template: &TemplateNode,
    ) -> Result<(), String> {
        let sig = client_signature(s);
        if sig == self.synced || self.app.state_mut().committing {
            return Ok(());
        }
        vf_fs::intake::prepare_filer_dir(
            &mut s.conn,
            &s.actor,
            &self.dir,
            &s.root,
            &s.cabinet,
            template,
            &default_inbox(&s.data),
        )?;
        let st = self.app.state_mut();
        st.template = template.clone();
        st.clients
            .load(ctx, self.dir.clone(), None, ColumnMap::default());
        let (rels, ok, _) = filer_core::relationships::load(&self.dir);
        if ok {
            st.relationships = rels;
            st.relationships_generation += 1;
        }
        self.synced = client_signature(s);
        Ok(())
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        eframe::App::ui(&mut self.app, ui, frame);
    }

    pub fn on_exit(&mut self) {
        eframe::App::on_exit(&mut self.app);
    }
}
