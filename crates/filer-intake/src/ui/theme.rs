//! Look and feel from UI_SPEC §1: colours, the IBM Plex fonts, text sizes, and the small pieces
//! every screen shares (pills, uppercase labels, cards, the primary button, the segmented
//! control). A dark variant follows the Windows setting (X122).

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, Response,
    RichText, Stroke, TextStyle, TextWrapMode, Theme, ThemePreference, Visuals,
};

/// Font family names for the semibold faces (egui has no font weights).
pub const SEMIBOLD: &str = "plex-sans-semibold";
pub const MONO_SEMIBOLD: &str = "plex-mono-semibold";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillKind {
    Ok,
    Warn,
    Err,
    Info,
    Neutral,
}

/// Every colour the screens use.
pub struct Palette {
    /// App background.
    pub paper: Color32,
    /// Cards, header, footer, fields.
    pub card: Color32,
    pub border: Color32,
    pub button_border: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    /// Amber: Conflicts badge, needs-attention field borders.
    pub attention: Color32,
    /// Needs-attention rows.
    pub tint_attention: Color32,
    /// Dangling / duplicate rows.
    pub tint_danger: Color32,
    /// Copy sub-rows, table header.
    pub tint_copy: Color32,
    /// (background, text) per [`PillKind`]: ok, warn, err, info, neutral.
    pills: [(Color32, Color32); 5],
}

impl Palette {
    pub fn pill(&self, kind: PillKind) -> (Color32, Color32) {
        self.pills[match kind {
            PillKind::Ok => 0,
            PillKind::Warn => 1,
            PillKind::Err => 2,
            PillKind::Info => 3,
            PillKind::Neutral => 4,
        }]
    }
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// UI_SPEC §1.
pub const LIGHT: Palette = Palette {
    paper: hex(0xF3F4F6),
    card: hex(0xFFFFFF),
    border: hex(0xD7DBE0),
    button_border: hex(0xC9CED5),
    hover: hex(0xF0F2F5),
    pressed: hex(0xE4E8EC),
    text: hex(0x1B1F24),
    muted: hex(0x5B6470),
    accent: hex(0x0F6B8E),
    on_accent: hex(0xFFFFFF),
    attention: hex(0xC98A1B),
    tint_attention: hex(0xFFFBF0),
    tint_danger: hex(0xFFF5F5),
    tint_copy: hex(0xF7F9FB),
    pills: [
        (hex(0xDFF2E6), hex(0x166A3F)),
        (hex(0xFCEBC5), hex(0x7A4B00)),
        (hex(0xF9E0E0), hex(0x9B2226)),
        (hex(0xE1ECF8), hex(0x1F4D7A)),
        (hex(0xEDF0F3), hex(0x4A525C)),
    ],
};

/// Not in the spec (X122): same hues on dark surfaces.
pub const DARK: Palette = Palette {
    paper: hex(0x16191D),
    card: hex(0x1F2328),
    border: hex(0x343A42),
    button_border: hex(0x46505A),
    hover: hex(0x2A3037),
    pressed: hex(0x323942),
    text: hex(0xE6E8EB),
    muted: hex(0x9AA3AE),
    accent: hex(0x4FA3C7),
    on_accent: hex(0x0B1A21),
    attention: hex(0xD9A040),
    tint_attention: hex(0x2A2416),
    tint_danger: hex(0x2E1D1F),
    tint_copy: hex(0x1B1F23),
    pills: [
        (hex(0x1E3A2A), hex(0x7FD3A0)),
        (hex(0x3D2F12), hex(0xF2C46B)),
        (hex(0x3D1C1E), hex(0xF19A9E)),
        (hex(0x1C2D42), hex(0x8DB8E8)),
        (hex(0x2A2F35), hex(0xB5BDC6)),
    ],
};

/// The palette for the theme `ui` is drawn in.
pub fn pal(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode {
        &DARK
    } else {
        &LIGHT
    }
}

/// Fonts, text sizes, spacing and both themes. Call once at startup.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    for (theme, p) in [(Theme::Light, &LIGHT), (Theme::Dark, &DARK)] {
        ctx.style_mut_of(theme, |style| {
            style.text_styles = [
                (
                    TextStyle::Small,
                    FontId::new(11.0, FontFamily::Proportional),
                ),
                (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
                (
                    TextStyle::Button,
                    FontId::new(13.0, FontFamily::Proportional),
                ),
                (
                    TextStyle::Heading,
                    FontId::new(15.0, FontFamily::Name(SEMIBOLD.into())),
                ),
                (
                    TextStyle::Monospace,
                    FontId::new(12.0, FontFamily::Monospace),
                ),
            ]
            .into();
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(12.0, 6.0);
            style.spacing.interact_size.y = 26.0;
            style.visuals = visuals(p, theme == Theme::Dark);
        });
    }
    ctx.set_theme(ThemePreference::System);
    // When Windows doesn't say, use the spec's light theme.
    ctx.options_mut(|o| o.fallback_theme = Theme::Light);
}

fn fonts() -> FontDefinitions {
    let mut f = FontDefinitions::default();
    let faces: [(&str, &'static [u8]); 4] = [
        (
            "plex-sans",
            include_bytes!("../../assets/fonts/IBMPlexSans-Regular.ttf"),
        ),
        (
            SEMIBOLD,
            include_bytes!("../../assets/fonts/IBMPlexSans-SemiBold.ttf"),
        ),
        (
            "plex-mono",
            include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf"),
        ),
        (
            MONO_SEMIBOLD,
            include_bytes!("../../assets/fonts/IBMPlexMono-SemiBold.ttf"),
        ),
    ];
    for (name, bytes) in faces {
        f.font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    // egui's own fonts stay as fallbacks for glyphs Plex lacks.
    let fallback_sans = f.families[&FontFamily::Proportional].clone();
    let fallback_mono = f.families[&FontFamily::Monospace].clone();
    let with = |first: &[&str], rest: &[String]| -> Vec<String> {
        first
            .iter()
            .map(|s| s.to_string())
            .chain(rest.iter().cloned())
            .collect()
    };
    f.families.insert(
        FontFamily::Proportional,
        with(&["plex-sans"], &fallback_sans),
    );
    f.families
        .insert(FontFamily::Monospace, with(&["plex-mono"], &fallback_mono));
    f.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        with(&[SEMIBOLD, "plex-sans"], &fallback_sans),
    );
    f.families.insert(
        FontFamily::Name(MONO_SEMIBOLD.into()),
        with(&[MONO_SEMIBOLD, "plex-mono"], &fallback_mono),
    );
    f
}

fn visuals(p: &Palette, dark: bool) -> Visuals {
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = p.paper;
    v.window_fill = p.card;
    v.extreme_bg_color = p.card;
    v.faint_bg_color = p.tint_copy;
    v.code_bg_color = p.pill(PillKind::Neutral).0;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.pill(PillKind::Warn).1;
    v.error_fg_color = p.pill(PillKind::Err).1;
    v.weak_text_color = Some(p.muted);
    v.selection.bg_fill = p.accent.gamma_multiply(0.25);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.striped = false;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.weak_bg_fill = p.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = CornerRadius::same(6);
    for (state, fill, stroke) in [
        (&mut w.inactive, p.card, p.button_border),
        (&mut w.hovered, p.hover, p.muted),
        (&mut w.active, p.pressed, p.accent),
        (&mut w.open, p.hover, p.button_border),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.0, p.text);
        state.corner_radius = CornerRadius::same(4);
        state.expansion = 0.0;
    }
    v
}

// ---------------------------------------------------------------------------
// Shared pieces

/// Semibold body text.
pub fn strong(text: impl Into<String>) -> RichText {
    RichText::new(text).family(FontFamily::Name(SEMIBOLD.into()))
}

/// Monospace text (paths, file names, IDs, patterns).
pub fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text).monospace()
}

/// Semibold monospace text.
pub fn mono_strong(text: impl Into<String>) -> RichText {
    RichText::new(text)
        .family(FontFamily::Name(MONO_SEMIBOLD.into()))
        .size(12.0)
}

/// A status pill: small bold text on a tinted background (UI_SPEC §1). It is a real label, so
/// tests and screen readers find it by its text.
pub fn pill(ui: &mut egui::Ui, kind: PillKind, text: impl Into<String>) -> Response {
    let (bg, fg) = pal(ui).pill(kind);
    Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::symmetric(6, 3))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(text)
                        .size(11.0)
                        .family(FontFamily::Name(SEMIBOLD.into()))
                        .color(fg),
                )
                .wrap_mode(TextWrapMode::Extend),
            )
        })
        .inner
}

/// A pill with monospace text, for `{Token}` names.
/// One widget of known size (not a Frame around a Label), so `horizontal_wrapped` can move it to
/// the next line instead of letting a row run past its column.
pub fn token_pill(ui: &mut egui::Ui, kind: PillKind, text: impl Into<String>) -> Response {
    let (bg, fg) = pal(ui).pill(kind);
    let text: String = text.into();
    let galley = egui::WidgetText::from(mono_strong(text.clone()).color(fg)).into_galley(
        ui,
        Some(TextWrapMode::Extend),
        f32::INFINITY,
        TextStyle::Body,
    );
    let pad = egui::vec2(6.0, 3.0);
    let (rect, response) = ui.allocate_exact_size(galley.size() + 2.0 * pad, egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, CornerRadius::same(3), bg);
        let pos = rect.center() - galley.size() / 2.0;
        ui.painter().galley(pos, galley, fg);
    }
    response
}

/// An 11px uppercase muted label (UI_SPEC §1 "labels").
pub fn label_caps(ui: &mut egui::Ui, text: &str) -> Response {
    let muted = pal(ui).muted;
    ui.label(
        RichText::new(text.to_uppercase())
            .size(11.0)
            .color(muted)
            .extra_letter_spacing(0.5),
    )
}

/// A white card with a 1px border and 6px corners.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let p = pal(ui);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(16))
        .show(ui, add)
        .inner
}

/// The filled accent button.
pub fn primary_button(ui: &egui::Ui, text: impl Into<String>) -> egui::Button<'static> {
    let p = pal(ui);
    egui::Button::new(strong(text).color(p.on_accent)).fill(p.accent)
}

/// Overwrite / Keep both / Skip style control: one button per option, the chosen one filled
/// with the accent. Returns `true` when the value changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    let p = pal(ui);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (opt, label) in options {
            let selected = *value == *opt;
            let text = if selected {
                strong(*label).color(p.on_accent)
            } else {
                RichText::new(*label)
            };
            let mut b = egui::Button::new(text).selected(selected);
            if selected {
                b = b.fill(p.accent);
            }
            if ui.add(b).clicked() && !selected {
                *value = *opt;
                changed = true;
            }
        }
    });
    changed
}

/// An on/off switch (UI_SPEC §7 "enabled switch"). Accessible as a checkbox named `label`.
pub fn switch(ui: &mut egui::Ui, on: &mut bool, label: &str) -> Response {
    let p = pal(ui);
    let size = egui::vec2(32.0, 18.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let value = *on;
    let name = label.to_string();
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), value, name.clone())
    });
    let t = ui.ctx().animate_bool(resp.id, value);
    let track = if value { p.accent } else { p.button_border };
    let track = if ui.is_enabled() {
        track
    } else {
        track.gamma_multiply(0.5)
    };
    ui.painter().rect_filled(rect, CornerRadius::same(9), track);
    let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), 7.0, p.card);
    resp
}

/// A smaller switch for table cells.
pub fn switch_small(ui: &mut egui::Ui, on: &mut bool, label: &str) -> Response {
    let p = pal(ui);
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(24.0, 14.0), egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let value = *on;
    let name = label.to_string();
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), value, name.clone())
    });
    let t = ui.ctx().animate_bool(resp.id, value);
    let track = if value { p.accent } else { p.button_border };
    ui.painter().rect_filled(rect, CornerRadius::same(7), track);
    let x = egui::lerp(rect.left() + 7.0..=rect.right() - 7.0, t);
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), 5.0, p.card);
    resp
}

/// A full-width message: error in the err pill colours, info in the info pill colours.
pub fn banner(ui: &mut egui::Ui, level: crate::config::Level, text: &str) {
    let kind = match level {
        crate::config::Level::Error => PillKind::Err,
        crate::config::Level::Info => PillKind::Info,
    };
    let (bg, fg) = pal(ui).pill(kind);
    Frame::new()
        .fill(bg)
        .inner_margin(Margin::symmetric(12, 8))
        .corner_radius(CornerRadius::same(4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(fg));
        });
    ui.add_space(4.0);
}

/// The last part of a path, for display.
pub fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `"s"` unless `n` is 1.
pub fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `512 B`, `2 KB`, `3.1 MB`.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{:.1} MB", b / KB / KB)
    }
}

/// `1284` → `1,284`.
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_plurals() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(3 * 1024 * 1024 + 100_000), "3.1 MB");
        assert_eq!((plural(1), plural(2)), ("", "s"));
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1284), "1,284");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn spec_colours() {
        assert_eq!(LIGHT.accent, Color32::from_rgb(0x0F, 0x6B, 0x8E));
        assert_eq!(
            LIGHT.pill(PillKind::Warn),
            (
                Color32::from_rgb(0xFC, 0xEB, 0xC5),
                Color32::from_rgb(0x7A, 0x4B, 0x00)
            )
        );
    }
}
