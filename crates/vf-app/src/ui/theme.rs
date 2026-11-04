//! Type and color tokens from UI_SPEC "Global chrome", light and dark (owner 2026-11-04: dark
//! mode). Colors are functions: they answer for the theme the window shows now ([`set_dark`],
//! called every frame from the window's theme). The dark palette is Filer Desktop's, so the
//! mounted Intake tab and Vibe's own tabs match.

use std::sync::atomic::{AtomicBool, Ordering};

use egui::Color32;

static DARK: AtomicBool = AtomicBool::new(false);

/// The window shows the dark theme (set every frame by the app).
pub fn set_dark(dark: bool) {
    DARK.store(dark, Ordering::Relaxed);
}

pub fn is_dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn pick(light: u32, dark: u32) -> Color32 {
    hex(if is_dark() { dark } else { light })
}

/// The header bar: dark in both themes.
pub const HEADER: Color32 = hex(0x161A20);
/// Muted text on the header.
pub const ON_DARK_MUTED: Color32 = hex(0xB5BCC6);

pub fn accent() -> Color32 {
    pick(0x1F56A8, 0x6EA8E8)
}
pub fn ink() -> Color32 {
    pick(0x161A20, 0xE6E8EB)
}
pub fn ink_secondary() -> Color32 {
    pick(0x3F4753, 0xC3C9D1)
}
pub fn ink_muted() -> Color32 {
    pick(0x5B6470, 0x9AA3AE)
}
pub fn app_bg() -> Color32 {
    pick(0xEEF0F3, 0x16191D)
}
pub fn pane() -> Color32 {
    pick(0xFFFFFF, 0x1F2328)
}
pub fn footer() -> Color32 {
    pick(0xF4F6F9, 0x1B1F23)
}
pub fn border_pane() -> Color32 {
    pick(0xD5D9E0, 0x343A42)
}
pub fn health_ok() -> Color32 {
    pick(0x2E7D4F, 0x7FD3A0)
}
/// Health "warn" dot / text that needs attention.
pub fn warn() -> Color32 {
    pick(0xB7791F, 0xD9A040)
}
/// Error text.
pub fn error() -> Color32 {
    pick(0xB42318, 0xF19A9E)
}
/// The Viewer's background around the pages.
pub fn canvas() -> Color32 {
    pick(0xDEE2E8, 0x2A2F35)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) * (1.0 - t) + f32::from(y) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// A tag's (background, text) designed for the light theme, made readable in the dark one: a
/// dark tint of the text color behind the light background color as text.
pub fn pill(bg: Color32, fg: Color32) -> (Color32, Color32) {
    if is_dark() {
        (mix(fg, pane(), 0.7), bg)
    } else {
        (bg, fg)
    }
}

/// A notice box fill designed for the light theme (a pale tint), in the dark one a dark tint.
pub fn soft(light: Color32) -> Color32 {
    if is_dark() {
        // The pale tint's hue made strong, then a little of it on the dark pane.
        let strong = |c: u8| 255u8.saturating_sub((255 - c).saturating_mul(6));
        let hue = Color32::from_rgb(strong(light.r()), strong(light.g()), strong(light.b()));
        mix(pane(), hue, 0.18)
    } else {
        light
    }
}

/// Filer Desktop's theme (fonts incl. the semibold faces its Intake screens use, text sizes,
/// spacing, both palettes) as the base, so the mounted Intake tab and Vibe's own tabs look the
/// same; then Vibe's tweaks for both themes.
pub fn install(ctx: &egui::Context) {
    // Vibe's own choice, not what Filer's install sets (it follows Windows).
    let dark = is_dark();
    filer_intake::ui::theme::install(ctx);
    for (theme, bg, text) in [
        (egui::Theme::Light, hex(0xEEF0F3), hex(0x161A20)),
        (egui::Theme::Dark, hex(0x16191D), hex(0xE6E8EB)),
    ] {
        ctx.style_mut_of(theme, |s| {
            s.visuals.panel_fill = bg;
            s.visuals.override_text_color = Some(text);
        });
    }
    set_theme(ctx, dark);
}

/// Shows the light or the dark theme.
pub fn set_theme(ctx: &egui::Context, dark: bool) {
    ctx.set_theme(if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    set_dark(dark);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dark palette differs from the light one, and tags stay readable in both.
    #[test]
    fn palettes_and_tags() {
        set_dark(false);
        let (light_ink, light_pane) = (ink(), pane());
        let (bg, fg) = pill(hex(0xFBEFD5), hex(0x7A4F07));
        assert_eq!((bg, fg), (hex(0xFBEFD5), hex(0x7A4F07)));
        set_dark(true);
        assert_ne!(ink(), light_ink);
        assert_ne!(pane(), light_pane);
        let (bg, fg) = pill(hex(0xFBEFD5), hex(0x7A4F07));
        // Light text on a dark tint.
        assert!(fg.r() > 200 && bg.r() < 120, "{bg:?} {fg:?}");
        set_dark(false);
    }
}
