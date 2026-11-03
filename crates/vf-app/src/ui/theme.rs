//! Type and color tokens from UI_SPEC "Global chrome".

use egui::Color32;

pub const ACCENT_DEFAULT: Color32 = Color32::from_rgb(0x1F, 0x56, 0xA8);
pub const INK: Color32 = Color32::from_rgb(0x16, 0x1A, 0x20);
pub const INK_SECONDARY: Color32 = Color32::from_rgb(0x3F, 0x47, 0x53);
pub const INK_MUTED: Color32 = Color32::from_rgb(0x5B, 0x64, 0x70);
pub const ON_DARK_MUTED: Color32 = Color32::from_rgb(0xB5, 0xBC, 0xC6);
pub const APP_BG: Color32 = Color32::from_rgb(0xEE, 0xF0, 0xF3);
pub const PANE: Color32 = Color32::WHITE;
pub const FOOTER: Color32 = Color32::from_rgb(0xF4, 0xF6, 0xF9);
pub const BORDER_PANE: Color32 = Color32::from_rgb(0xD5, 0xD9, 0xE0);
pub const HEALTH_OK: Color32 = Color32::from_rgb(0x2E, 0x7D, 0x4F);
/// Health "warn" dot / text that needs attention.
pub const WARN: Color32 = Color32::from_rgb(0xB7, 0x79, 0x1F);

/// Filer Desktop's theme (fonts incl. the semibold faces its Intake screens use, text sizes,
/// spacing, both palettes) as the base, so the mounted Intake tab and Vibe's own tabs look the
/// same; then Vibe's tweaks, light only.
pub fn install(ctx: &egui::Context) {
    filer_intake::ui::theme::install(ctx);
    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |s| {
        s.visuals.panel_fill = APP_BG;
        s.visuals.override_text_color = Some(INK);
    });
}
