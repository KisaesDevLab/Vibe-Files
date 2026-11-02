//! Type and color tokens from UI_SPEC "Global chrome".
// The full palette from UI_SPEC; tabs pick tokens up as they are built.
#![allow(dead_code)]

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
pub const HEALTH_WARN: Color32 = Color32::from_rgb(0xB7, 0x79, 0x1F);
pub const BORDER_ROW: Color32 = Color32::from_rgb(0xE9, 0xEC, 0xF0);
pub const HOVER: Color32 = Color32::from_rgb(0xF4, 0xF6, 0xF9);
pub const SELECT_BG: Color32 = Color32::from_rgb(0xE6, 0xEE, 0xF9);
pub const FOLDER_CLIENT: Color32 = Color32::from_rgb(0xD9, 0xB4, 0x6B);
pub const FOLDER_SUB: Color32 = Color32::from_rgb(0xE6, 0xD3, 0xA3);
// Status pills (bg, fg).
pub const NEUTRAL_BG: Color32 = Color32::from_rgb(0xEE, 0xF0, 0xF3);
pub const WARN_BG: Color32 = Color32::from_rgb(0xFB, 0xEF, 0xD5);
pub const WARN_FG: Color32 = Color32::from_rgb(0x7A, 0x4F, 0x07);
pub const OK_BG: Color32 = Color32::from_rgb(0xDD, 0xF2, 0xE4);
pub const OK_FG: Color32 = Color32::from_rgb(0x1D, 0x5C, 0x38);
pub const ERR_BG: Color32 = Color32::from_rgb(0xFB, 0xE3, 0xE3);
pub const ERR_FG: Color32 = Color32::from_rgb(0x9A, 0x1B, 0x1B);

/// Vibe Files' look on top of Filer Desktop's theme. Filer's fonts are the full set (IBM Plex
/// Sans / SemiBold / Mono / Mono SemiBold, plus egui's fonts as fallbacks for glyphs Plex lacks)
/// and the Intake tab's screens need its named families, so they are never replaced; only the
/// spec's light theme and surfaces are applied over it.
pub fn install(ctx: &egui::Context) {
    filer_intake::ui::theme::install(ctx);
    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |s| {
        s.visuals.panel_fill = APP_BG;
        s.visuals.override_text_color = Some(INK);
    });
}
