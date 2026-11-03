//! UI test helpers shared by the tab tests.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

/// Clicks the button labelled `label`, then runs until the UI settles.
pub fn click<S>(h: &mut Harness<'_, S>, label: &str) {
    h.get_by_role_and_label(Role::Button, label).click();
    h.run();
}

/// Focuses the field labelled `label` and types `text` (appended to what is there).
pub fn type_into<S>(h: &mut Harness<'_, S>, label: &str, text: &str) {
    h.get_by_label(label).click();
    h.step();
    h.get_by_label(label).type_text(text);
    h.step();
}

/// Binds the process-wide pdfium before a test starts anything that looks for it. The vendored
/// `filer_pdf::pdf::find_pdfium` test-binds the library; if that overlaps the first
/// `filer_pdf::pdf::pdfium` call on another test thread, the "already initialized" error is
/// cached for the whole test process. The app binds once at startup, so only tests overlap.
pub fn pdfium_first() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Some(dir) = filer_pdf::pdf::find_pdfium(std::path::Path::new("")) {
            let _ = filer_pdf::pdf::pdfium(&dir);
        }
    });
}
