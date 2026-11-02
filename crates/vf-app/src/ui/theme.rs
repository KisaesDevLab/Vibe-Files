//! Type and color tokens from UI_SPEC "Global chrome".
// The full palette from UI_SPEC; tabs pick tokens up as they are built.
#![allow(dead_code)]

use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily};

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

pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |s| {
        s.visuals.panel_fill = APP_BG;
        s.visuals.override_text_color = Some(INK);
    });
}

fn fonts() -> FontDefinitions {
    let mut f = FontDefinitions::default();
    let faces: [(&str, &'static [u8]); 2] = [
        (
            "plex-sans",
            include_bytes!("../../assets/fonts/IBMPlexSans-Regular.ttf"),
        ),
        (
            "plex-mono",
            include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf"),
        ),
    ];
    for (name, bytes) in faces {
        f.font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    for (family, face) in [
        (FontFamily::Proportional, "plex-sans"),
        (FontFamily::Monospace, "plex-mono"),
    ] {
        f.families
            .entry(family)
            .or_default()
            .insert(0, face.to_owned());
    }
    f
}
