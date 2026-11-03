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
