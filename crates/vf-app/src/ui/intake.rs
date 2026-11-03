//! Intake tab (P02 step 1): the vendored Filer Desktop app (`filer_intake::app::FilerApp`), mounted
//! unchanged.
//!
//! Filer keeps all of its files in one data folder. Vibe Files points it at `<cabinet>/_Vibe/`
//! (QUESTIONS Q-19): `clients.json`, `relationships.json`, `template.json` there are the cabinet's
//! client master already, so Filer and Browse share one master. Filer's other files
//! (`config.json`, `rules.json`, `folders.json`, `last_batch.json`, `ops.jsonl`, `tmp/`,
//! `overwritten/`, `undo_zips/`, `split_originals/`) are cabinet-level too, which is safe only
//! with one writer at a time: two PCs must not run Intake on one cabinet together until the P09
//! catalog-writer lock gates it.

use std::path::{Path, PathBuf};

use filer_intake::app::{AppState, FilerApp};
use filer_intake::config::{Settings, ThemeChoice};
use vf_fs::cabinet::CabinetFile;

use crate::ui::theme;

#[derive(Default)]
pub struct IntakeTab {
    /// Built the first time the tab is shown (its `new` starts the inbox watcher and the
    /// first-page sniffer threads).
    filer: Option<FilerApp>,
    error: Option<String>,
}

/// Filer's settings for this cabinet on this PC: whatever `_Vibe/config.json` holds (rules
/// profile, inboxes, column map, …), with the cabinet's own facts written over it.
pub fn settings_for(root: &Path, cabinet: &CabinetFile) -> Result<(Settings, bool), String> {
    let dir = vf_fs::clients::master_dir(root);
    let (mut s, savable) = Settings::load_or_default(&dir);
    let before = serde_json::to_value(&s).ok();
    // The root as this PC opened it (a mapped drive here can be a UNC path elsewhere).
    s.root = Some(root.to_path_buf());
    // Every sentinel Filer writes must carry the cabinet's firm id.
    s.firm_id = Some(
        uuid::Uuid::parse_str(&cabinet.firm_id)
            .map_err(|e| format!("cabinet.json firm_id is not a uuid: {e}"))?,
    );
    // Vibe Files is light-only until it has a dark theme; Filer would otherwise follow Windows.
    s.display.theme = ThemeChoice::Light;
    // Filer re-imports the client CSV on launch when it changed; point it at the one the master
    // list came from.
    if let Ok(Some(list)) = filer_core::clients::load_cache(&dir)
        && !list.source.as_os_str().is_empty()
        && list.source.is_file()
    {
        s.csv_path = Some(list.source.clone());
        s.column_map = list.column_map.clone();
    }
    if savable && serde_json::to_value(&s).ok() != before {
        s.save(&dir)
            .map_err(|e| format!("saving Filer's config.json: {e}"))?;
    }
    Ok((s, savable))
}

/// The folder holding a `pdfium.dll` that binds (next to the exe; debug builds also find the
/// vendored copy), or `None` (Filer's PDF features off).
fn pdfium_dir() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    filer_pdf::pdf::find_pdfium(&exe_dir)
}

impl IntakeTab {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        frame: &mut eframe::Frame,
        root: &Path,
        cabinet: &CabinetFile,
    ) {
        if self.filer.is_none() && self.error.is_none() {
            match settings_for(root, cabinet) {
                Ok((settings, savable)) => {
                    let state = AppState::load(
                        vf_fs::clients::master_dir(root),
                        settings,
                        savable,
                        pdfium_dir(),
                    );
                    let ctx = ui.ctx().clone();
                    self.filer = Some(FilerApp::new(&ctx, state));
                    // Filer's `new` installs its theme; put Vibe's tweaks back on top.
                    theme::install(&ctx);
                }
                Err(e) => self.error = Some(e),
            }
        }
        match (&mut self.filer, &self.error) {
            (Some(f), _) => eframe::App::ui(f, ui, frame),
            (None, Some(e)) => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.heading("Intake");
                    ui.label(egui::RichText::new(e).color(crate::ui::wizard::ERROR));
                });
            }
            (None, None) => {}
        }
    }
}
