//! Top-level app: tab routing over the shared chrome. Each tab is a stub until its phase.

use crate::ui::{chrome, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Browse,
    Intake,
    Search,
    Viewer,
    Sync,
    Admin,
}

impl Tab {
    /// CLAUDE.md conventions: tabs in exactly this order.
    pub const ALL: [Tab; 6] = [
        Tab::Browse,
        Tab::Intake,
        Tab::Search,
        Tab::Viewer,
        Tab::Sync,
        Tab::Admin,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Browse => "Browse",
            Tab::Intake => "Intake",
            Tab::Search => "Search",
            Tab::Viewer => "Viewer",
            Tab::Sync => "Sync",
            Tab::Admin => "Admin",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Tab::Browse => "Create your first client or run the import wizard.",
            Tab::Intake => "Intake mounts the vendored Filer Desktop engine (P02).",
            Tab::Search => "Full-text search arrives with OCR and indexing (P03).",
            Tab::Viewer => "The PDF viewer arrives in P04.",
            Tab::Sync => "B2 sync arrives in P11.",
            Tab::Admin => "Users, doc types, audit and compliance settings.",
        }
    }
}

pub struct VibeFilesApp {
    pub tab: Tab,
}

impl VibeFilesApp {
    pub fn new(ctx: &egui::Context) -> Self {
        theme::install(ctx);
        Self { tab: Tab::Browse }
    }
}

impl eframe::App for VibeFilesApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        chrome::header(ui);
        chrome::tab_bar(ui, &mut self.tab, 0);
        chrome::footer(
            ui,
            &[format!("vibe-files {}", env!("CARGO_PKG_VERSION"))],
            "No cabinet open",
        );
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading(self.tab.name());
            ui.label(self.tab.placeholder());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::HarnessBuilder;
    use egui_kittest::kittest::Queryable;

    fn harness() -> egui_kittest::Harness<'static, VibeFilesApp> {
        HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(|cc| VibeFilesApp::new(&cc.egui_ctx))
    }

    #[test]
    fn six_tabs_in_spec_order() {
        let names: Vec<_> = Tab::ALL.iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            ["Browse", "Intake", "Search", "Viewer", "Sync", "Admin"]
        );
    }

    #[test]
    fn opens_on_browse_and_every_tab_switches() {
        let mut h = harness();
        h.run();
        assert_eq!(h.state().tab, Tab::Browse);
        for t in Tab::ALL {
            h.get_by_role_and_label(egui::accesskit::Role::Button, t.name())
                .click();
            h.run();
            assert_eq!(h.state().tab, t);
        }
    }
}
