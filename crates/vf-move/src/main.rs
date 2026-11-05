//! vibe-move.exe (PA-35): moves a client tree's Permanent folders, recently modified files and
//! files with a recent year into a new folder. Ships beside vibe-files.exe; runs on its own.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Vibe Move")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Vibe Move",
        options,
        Box::new(|_| Ok(Box::new(Window(app::MoveApp::new())))),
    )
}

struct Window(app::MoveApp);

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| self.0.ui(ui));
    }
}
