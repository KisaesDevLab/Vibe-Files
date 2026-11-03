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
use vf_core::db::Connection;
use vf_fs::cabinet::CabinetFile;

use crate::config::Ingest;
use crate::ui::ingest::IngestState;
use crate::ui::theme;
use vf_core::browse::Snapshot;

#[derive(Default)]
pub struct IntakeTab {
    /// Built the first time the tab is shown (its `new` starts the inbox watcher and the
    /// first-page sniffer threads).
    filer: Option<FilerApp>,
    error: Option<String>,
    /// `last_batch.json`'s modification time when the catalog was last brought in line with it.
    seen: Option<std::time::SystemTime>,
    /// The catalog changed (a batch was recorded or undone): Browse re-reads it.
    pub catalog_changed: bool,
    /// What the last reconcile did or why it failed, for Browse's notice.
    pub message: Option<String>,
    /// Vibe's staging folders and barcode sheets, shown above Filer.
    pub ingest: IngestState,
    /// Documents Filer just filed whose content was already in the cabinet (Q-27).
    pub dupes: Vec<vf_fs::dupes::Dup>,
    /// A choice made on one of `dupes`, applied on the next reconcile (it needs the catalog).
    dupe_action: Option<(usize, vf_fs::dupes::Choice)>,
}

/// Filer's ID pattern for client IDs of letters and/or digits (`STOC1234`, `1042`).
pub const ID_PATTERN: &str = r"[A-Z0-9]{4,}";

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
    // The office's UltraTax client IDs are letters and digits; Filer's default ID pattern is
    // digits only. A pattern the firm chose is kept (owner 2026-11-03).
    if s.id_pattern == filer_core::parse::DEFAULT_ID_PATTERN {
        s.id_pattern = ID_PATTERN.into();
    }
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
pub(crate) fn pdfium_dir() -> Option<PathBuf> {
    let exe_dir = vf_core::exe_dir()?;
    filer_pdf::pdf::find_pdfium(&exe_dir)
}

impl IntakeTab {
    /// Records Filer's last batch in the catalog when `last_batch.json` changed since the last
    /// look (a stat per frame otherwise). See `vf_fs::intake`.
    pub fn reconcile(&mut self, conn: &mut Connection, root: &Path, device_id: &str) {
        self.apply_dupe_action(conn, root, device_id);
        let file = filer_intake::commit::last_batch_path(&vf_fs::clients::master_dir(root));
        let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
        if modified.is_none() || modified == self.seen {
            return;
        }
        let before = vf_core::now_rfc3339();
        match vf_fs::intake::reconcile(conn, root, device_id) {
            Ok(r) => {
                self.seen = modified;
                if let Some((_, added, updated)) = r.recorded {
                    self.message = Some(format!(
                        "Intake filed {added} new documents{}.",
                        if updated > 0 {
                            format!(" and replaced {updated}")
                        } else {
                            String::new()
                        }
                    ));
                    match vf_fs::dupes::filed_since(conn, &before) {
                        Ok(d) => self.dupes.extend(d),
                        Err(e) => self.message = Some(format!("Duplicate check failed: {e}")),
                    }
                } else if r.undone.is_some() {
                    self.message = Some("Intake's last batch was undone.".into());
                }
                self.catalog_changed |= r.changed();
            }
            // Try again next frame (e.g. a file still being written); keep the message.
            Err(e) => self.message = Some(format!("Intake batch not recorded yet: {e}")),
        }
    }

    fn apply_dupe_action(&mut self, conn: &mut Connection, root: &Path, device_id: &str) {
        let Some((i, choice)) = self.dupe_action.take() else {
            return;
        };
        if i >= self.dupes.len() {
            return;
        }
        let d = self.dupes.remove(i);
        match vf_fs::dupes::resolve(conn, root, device_id, &d, choice) {
            Ok(_) => {
                self.catalog_changed = true;
                self.message = Some(match choice {
                    vf_fs::dupes::Choice::RemoveNew => format!("Moved {} to the trash.", d.new_rel),
                    vf_fs::dupes::Choice::ReplaceOld => {
                        format!("Moved {} to the trash.", d.old_rel)
                    }
                    vf_fs::dupes::Choice::KeepBoth => format!("Kept both copies of {}.", d.new_rel),
                });
            }
            Err(e) => {
                self.message = Some(format!("Could not resolve the duplicate: {e}"));
                self.dupes.insert(i, d);
            }
        }
    }

    /// Filed duplicates, one line each with the three choices.
    fn dupes_panel(&mut self, ui: &mut egui::Ui) {
        if self.dupes.is_empty() {
            return;
        }
        egui::Panel::top("vf-dupes")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(0xFF, 0xF6, 0xE0))
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Already in the cabinet").strong());
                for (i, d) in self.dupes.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} has the same content as {}.", d.new_rel, d.old_rel));
                        use vf_fs::dupes::Choice;
                        for (label, c) in [
                            ("Remove new copy", Choice::RemoveNew),
                            ("Keep both", Choice::KeepBoth),
                            ("Replace old", Choice::ReplaceOld),
                        ] {
                            if ui.button(label).clicked() {
                                self.dupe_action = Some((i, c));
                            }
                        }
                    });
                }
                ui.label(crate::ui::widgets::caption(
                    "Removed copies go to the cabinet trash. Removing a file from Filer's last batch makes that batch's Undo unavailable.",
                ));
            });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        frame: &mut eframe::Frame,
        root: &Path,
        cabinet: &CabinetFile,
        prefs: &Ingest,
        snap: &Snapshot,
        sheets_dir: &Path,
    ) {
        self.ingest.strip(ui, prefs, snap, sheets_dir, root);
        self.dupes_panel(ui);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Client IDs of letters and digits match; Filer's digits-only default is replaced once,
    /// a pattern the firm set is kept.
    #[test]
    fn id_pattern_takes_letters_and_digits_unless_the_firm_set_one() {
        assert!(filer_core::parse::validate_id_pattern(ID_PATTERN).is_ok());
        let re = filer_core::parse::filename_regex(ID_PATTERN).unwrap();
        let c = re
            .captures("Anderson Farms LLC_ANDE1042_2025 UltraTax CS Return.pdf")
            .unwrap();
        assert_eq!(&c["id"], "ANDE1042");
        assert_eq!(
            &re.captures("Smith John_1042_2025 1040.pdf").unwrap()["id"],
            "1042"
        );

        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let cabinet = CabinetFile {
            cabinet_id: vf_core::new_id(),
            firm_id: uuid::Uuid::new_v4().to_string(),
            schema_version: vf_core::db::SCHEMA_VERSION,
            layout_version: 1,
            created: vf_core::now_rfc3339(),
            name: None,
        };
        vf_fs::cabinet::create(root, &cabinet).unwrap();
        let (s, _) = settings_for(root, &cabinet).unwrap();
        assert_eq!(s.id_pattern, ID_PATTERN);
        let dir = vf_fs::clients::master_dir(root);
        let (mut own, _) = Settings::load_or_default(&dir);
        assert_eq!(own.id_pattern, ID_PATTERN, "saved");
        own.id_pattern = r"\d{5}".into();
        own.save(&dir).unwrap();
        assert_eq!(settings_for(root, &cabinet).unwrap().0.id_pattern, r"\d{5}");
    }
}
