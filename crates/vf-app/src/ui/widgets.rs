//! Small shared widgets (UI_SPEC "egui mapping notes": implement once, reuse everywhere).

use egui::{Color32, CornerRadius, RichText};
use vf_fs::docmeta::STATUSES;

/// A rounded status pill.
pub fn pill(ui: &mut egui::Ui, text: &str, bg: Color32, fg: Color32) -> egui::Response {
    // Colors are given for the light theme; the dark one gets a dark tint (owner 2026-11-04).
    let (bg, fg) = crate::ui::theme::pill(bg, fg);
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| ui.label(RichText::new(text).small().color(fg)))
        .response
}

/// Document status: catalog value → (label, bg, fg). Labels come from
/// `vf_fs::docmeta::STATUSES`; colors are UI_SPEC's status pills.
pub fn status_style(status: &str) -> (&'static str, Color32, Color32) {
    let label = label_of(&STATUSES, Some(status), "Received");
    let (bg, fg) = match status {
        "in_work" => (rgb(0xFBEFD5), rgb(0x7A4F07)),
        "final" => (rgb(0xDDF2E4), rgb(0x1D5C38)),
        "delivered" => (rgb(0xE6EEF9), rgb(0x1F56A8)),
        _ => (rgb(0xEEF0F3), rgb(0x3F4753)),
    };
    (label, bg, fg)
}

/// Sort order of a status: its place in `STATUSES` (unknown values last).
pub fn status_rank(status: &str) -> usize {
    STATUSES
        .iter()
        .position(|(k, _)| *k == status)
        .unwrap_or(STATUSES.len())
}

/// The label for `key` in a `(key, label)` table, or `none`.
pub fn label_of(
    table: &[(&str, &'static str)],
    key: Option<&str>,
    none: &'static str,
) -> &'static str {
    key.and_then(|k| table.iter().find(|(kk, _)| *kk == k))
        .map(|(_, l)| *l)
        .unwrap_or(none)
}

/// The file name of a cabinet-relative path (forward slashes).
pub fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// A tax year for display; `None` is a permanent file.
pub fn year_label(y: Option<i32>) -> String {
    y.map(|y| y.to_string())
        .unwrap_or_else(|| "Permanent".into())
}

/// Small muted text: field captions, column headers, hints (UI_SPEC "labels 11px").
pub fn caption(text: impl Into<String>) -> RichText {
    RichText::new(text)
        .small()
        .color(crate::ui::theme::ink_muted())
}

/// The accent-filled primary button.
pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    let on_accent = if crate::ui::theme::is_dark() {
        crate::ui::theme::HEADER
    } else {
        Color32::WHITE
    };
    egui::Button::new(RichText::new(text).color(on_accent)).fill(crate::ui::theme::accent())
}

/// A fixed-size scrolling body for a modal. A modal that sizes itself to its content re-centers
/// over several frames and moves its buttons from under the pointer, so dialog bodies whose
/// content changes (previews, messages) must not change the modal's size.
pub fn fixed_body<R>(
    ui: &mut egui::Ui,
    id: &str,
    height: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .min_scrolled_height(height)
        .max_height(height)
        .auto_shrink([false, false])
        .show(ui, add)
        .inner
}

/// A fixed-height error line for a modal (empty when there is no message), for the same reason
/// as [`fixed_body`].
pub fn message_line(ui: &mut egui::Ui, msg: Option<&str>) {
    ui.add_sized(
        [ui.available_width(), 36.0],
        egui::Label::new(RichText::new(msg.unwrap_or("")).color(crate::ui::theme::error())).wrap(),
    );
}

/// A drop-down with a search box, for long lists (clients, doc types; owner 2026-11-05).
///
/// Opening it puts the cursor in **Type to search**: every word typed must appear in an item
/// (any case, anywhere: `smith 1001`). **Enter** picks the first match; a click picks any.
/// `items` gives the choices' labels (called only while the list is open); `current` the one chosen; `none` an extra first choice
/// meaning "nothing" ("Not set", "Any client"), offered while nothing is typed. `shown` is the
/// button's text. Returns the button and, when a choice was made, the new choice (`None` =
/// the `none` choice).
pub fn search_combo(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    width: f32,
    shown: impl Into<String>,
    items: impl FnOnce() -> Vec<String>,
    current: Option<usize>,
    none: Option<&str>,
) -> (egui::Response, Option<Option<usize>>) {
    let key = ui.make_persistent_id(("search-combo", &salt));
    let (q_id, focus_id) = (key.with("q"), key.with("focused"));
    let mut pick: Option<Option<usize>> = None;
    let r = egui::ComboBox::from_id_salt(&salt)
        .width(width)
        .selected_text(shown.into())
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            ui.set_min_width(width.max(240.0));
            let items = items();
            let mut q: String = ui.data(|d| d.get_temp(q_id)).unwrap_or_default();
            let te = ui.add(
                egui::TextEdit::singleline(&mut q)
                    .hint_text("Type to search")
                    .desired_width(f32::INFINITY),
            );
            // Named for screen readers (a hint is not a name).
            te.widget_info(|| {
                egui::WidgetInfo::labeled(egui::accesskit::Role::TextInput, true, "Type to search")
            });
            if !ui.data(|d| d.get_temp::<bool>(focus_id).unwrap_or(false)) {
                te.request_focus();
                ui.data_mut(|d| d.insert_temp(focus_id, true));
            }
            let words: Vec<String> = q
                .to_lowercase()
                .split_whitespace()
                .map(String::from)
                .collect();
            ui.data_mut(|d| d.insert_temp(q_id, q));
            let hits: Vec<usize> = (0..items.len())
                .filter(|&i| {
                    let l = items[i].to_lowercase();
                    words.iter().all(|w| l.contains(w.as_str()))
                })
                .collect();
            if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                // Nothing typed: the first row shown, which is the `none` choice when offered.
                pick = match (words.is_empty() && none.is_some(), hits.first()) {
                    (true, _) => Some(None),
                    (false, Some(&h)) => Some(Some(h)),
                    (false, None) => None,
                };
            }
            if !words.is_empty() {
                ui.label(caption(format!("{} of {}", hits.len(), items.len())));
            }
            if let Some(n) = none.filter(|_| words.is_empty())
                && ui.selectable_label(current.is_none(), n).clicked()
            {
                pick = Some(None);
            }
            let row_h = ui.spacing().interact_size.y;
            egui::ScrollArea::vertical()
                .id_salt(key.with("list"))
                .max_height(320.0)
                .auto_shrink([false, true])
                .show_rows(ui, row_h, hits.len(), |ui, range| {
                    for &i in &hits[range] {
                        if ui.selectable_label(current == Some(i), &items[i]).clicked() {
                            pick = Some(Some(i));
                        }
                    }
                });
            if pick.is_some() {
                ui.close();
            }
        });
    // Shut: the next opening starts with an empty search, cursor in it.
    if !egui::ComboBox::is_open(ui.ctx(), r.response.id) || pick.is_some() {
        ui.data_mut(|d| {
            d.remove::<String>(q_id);
            d.remove::<bool>(focus_id);
        });
    }
    let mut resp = r.response;
    if pick.is_some_and(|p| p != current) {
        resp.mark_changed();
    }
    (resp, pick)
}

/// What a modal dialog did this frame.
pub enum DialogOutcome<T> {
    Open,
    Cancelled,
    Done(T),
}

pub fn status_pill(ui: &mut egui::Ui, status: &str) -> egui::Response {
    let (label, bg, fg) = status_style(status);
    pill(ui, label, bg, fg)
}

/// Lock pill colors.
pub fn lock_pill(ui: &mut egui::Ui, user: &str) -> egui::Response {
    pill(
        ui,
        &format!("Locked · {user}"),
        rgb(0xFBEFD5),
        rgb(0x7A4F07),
    )
}

pub const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// `1.2 MB`, `84 KB`, `512 B`.
pub fn size(bytes: i64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b >= K * K * K {
        format!("{:.1} GB", b / (K * K * K))
    } else if b >= K * K {
        format!("{:.1} MB", b / (K * K))
    } else if b >= K {
        format!("{:.0} KB", b / K)
    } else {
        format!("{bytes} B")
    }
}

/// Storage time (RFC 3339 UTC) shown in local time, `2026-11-02 14:05`.
pub fn local_time(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(86_000), "84 KB");
        assert_eq!(size(1_258_291), "1.2 MB");
    }

    #[test]
    fn bad_time_shown_as_is() {
        assert_eq!(local_time("not a time"), "not a time");
        assert_eq!(local_time("2026-11-02T14:05:00Z").len(), 16);
    }

    /// The searchable drop-down (owner 2026-11-05): typing narrows a long list, Enter picks
    /// the first match, the `none` choice is offered while nothing is typed.
    #[test]
    fn search_combo_narrows_and_picks() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::HarnessBuilder::default()
            .with_size([600.0, 600.0])
            .build_ui_state(
                |ui, chosen: &mut Option<usize>| {
                    let names: Vec<String> =
                        (0..500).map(|i| format!("Client {i:03} (ID{i})")).collect();
                    let shown = chosen.map_or("Any client".to_string(), |i| names[i].clone());
                    let l = ui.label("Client");
                    let (r, pick) = search_combo(
                        ui,
                        "t",
                        220.0,
                        shown,
                        || names.clone(),
                        *chosen,
                        Some("Any client"),
                    );
                    r.labelled_by(l.id);
                    if let Some(p) = pick {
                        *chosen = p;
                    }
                },
                None,
            );
        crate::ui::theme::install(&h.ctx);
        h.run();
        h.get_by_role_and_label(Role::ComboBox, "Client").click();
        h.run();
        assert!(h.query_by_label("Any client").is_some());
        h.get_by_label("Type to search").type_text("id42 client 4");
        h.run();
        assert!(
            h.query_by_label("11 of 500").is_some(),
            "Client 042 and 420-429"
        );
        assert!(h.query_by_label("Client 042 (ID42)").is_some());
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(*h.state(), Some(42));
        // Opened again: the search starts empty.
        h.get_by_role_and_label(Role::ComboBox, "Client").click();
        h.run();
        assert!(h.query_by_label("Any client").is_some());
        h.get_by_label("Any client").click();
        h.run();
        assert_eq!(*h.state(), None);
        // Esc closes it, nothing chosen.
        h.get_by_role_and_label(Role::ComboBox, "Client").click();
        h.run();
        assert!(h.query_by_label("Type to search").is_some());
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(h.query_by_label("Type to search").is_none(), "closed");
        assert_eq!(*h.state(), None);
    }
}
