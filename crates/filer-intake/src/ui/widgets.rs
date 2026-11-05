//! Widgets shared by several tabs.

use std::path::Path;

use crate::clients::{Client, ClientList};

/// Opens Explorer with `path` selected (or the folder itself, or its parent if it is gone).
pub fn show_in_explorer(path: &Path) {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("explorer");
    if path.is_file() {
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
    } else if let Some(dir) = [Some(path), path.parent()]
        .into_iter()
        .flatten()
        .find(|d| d.is_dir())
    {
        cmd.arg(dir);
    } else {
        return;
    }
    if let Err(e) = cmd.spawn() {
        tracing::warn!(error = %e, "cannot open Explorer");
    }
}

/// Inside an open menu: a "Type to search" box (the cursor in it when the menu opens) over a
/// scrolled list of `items`. Every word typed must appear in an item (any case, any order);
/// Enter picks the first match. Returns the item picked; the caller closes the menu.
pub fn search_list(
    ui: &mut egui::Ui,
    key: egui::Id,
    items: &[String],
    current: Option<usize>,
) -> Option<usize> {
    let (q_id, seen_id) = (key.with("q"), key.with("seen"));
    // A menu shown again after a frame shut starts empty, with the cursor in the box.
    let pass = ui.ctx().cumulative_pass_nr();
    let fresh = ui
        .data(|d| d.get_temp::<u64>(seen_id))
        .is_none_or(|seen| seen + 1 < pass);
    ui.data_mut(|d| d.insert_temp(seen_id, pass));
    let mut q: String = if fresh {
        String::new()
    } else {
        ui.data(|d| d.get_temp(q_id)).unwrap_or_default()
    };
    let te = ui.add(
        egui::TextEdit::singleline(&mut q)
            .hint_text("Type to search")
            .desired_width(f32::INFINITY),
    );
    te.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Type to search"));
    if fresh {
        te.request_focus();
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
    let mut pick = None;
    if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        // Nothing typed: keep what is chosen (nothing for a menu with no current choice).
        pick = if words.is_empty() {
            current
        } else {
            hits.first().copied()
        };
    }
    if !words.is_empty() {
        ui.label(
            egui::RichText::new(format!("{} of {}", hits.len(), items.len()))
                .small()
                .weak(),
        );
    }
    let row_h = ui.spacing().interact_size.y;
    egui::ScrollArea::vertical()
        .id_salt(key.with("list"))
        .max_height(360.0)
        .auto_shrink([false, true])
        .show_rows(ui, row_h, hits.len(), |ui, range| {
            for &i in &hits[range] {
                if ui.selectable_label(current == Some(i), &items[i]).clicked() {
                    pick = Some(i);
                }
            }
        });
    pick
}

pub enum PickerOutcome {
    Open,
    Cancel,
    Chosen(Client),
    /// "New client…" (only from [`client_picker_or_new`]), with the search text.
    NewClient(String),
}

/// Clients matching `query`: an exact ID first (score `None`), then the best name matches.
pub fn search_clients<'a>(
    clients: &'a ClientList,
    query: &str,
    limit: usize,
) -> Vec<(Option<f64>, &'a Client)> {
    let q = query.trim();
    let mut results: Vec<(Option<f64>, &Client)> = Vec::new();
    if q.is_empty() {
        return results;
    }
    if let Some(c) = clients.match_by_id(q) {
        results.push((None, c));
    }
    for (score, c) in clients.fuzzy_by_name(q, limit) {
        if !results.iter().any(|(_, r)| r.id == c.id) {
            results.push((Some(score), c));
        }
    }
    results
}

/// Modal "pick a client" dialog: search by name or ID, best matches first.
pub fn client_picker(
    ctx: &egui::Context,
    title: &str,
    query: &mut String,
    clients: &ClientList,
) -> PickerOutcome {
    client_picker_or_new(ctx, title, query, clients, false)
}

/// [`client_picker`], with "New client…" when `offer_new` (a host that creates clients).
pub fn client_picker_or_new(
    ctx: &egui::Context,
    title: &str,
    query: &mut String,
    clients: &ClientList,
    offer_new: bool,
) -> PickerOutcome {
    let mut outcome = PickerOutcome::Open;
    let modal = egui::Modal::new(egui::Id::unique("client_picker")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.heading(title);
        ui.add_space(6.0);
        let label = ui.label("Search by name or ID");
        ui.add(egui::TextEdit::singleline(query).desired_width(f32::INFINITY))
            .labelled_by(label.id);
        ui.add_space(6.0);

        let results = search_clients(clients, query, 10);
        if results.is_empty() {
            ui.label("Type a client name or ID.");
        }
        for (score, c) in results {
            let text = match score {
                Some(s) => format!("{} ({})  {:.2}", c.name, c.id, s),
                None => format!("{} ({})  ID match", c.name, c.id),
            };
            if ui.button(text).clicked() {
                outcome = PickerOutcome::Chosen(c.clone());
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if offer_new
                && ui
                    .button("New client…")
                    .on_hover_text("Not in the list: create the client")
                    .clicked()
            {
                outcome = PickerOutcome::NewClient(query.trim().to_string());
            }
            if ui.button("Cancel").clicked() {
                outcome = PickerOutcome::Cancel;
            }
        });
    });
    if matches!(outcome, PickerOutcome::Open) && modal.should_close() {
        outcome = PickerOutcome::Cancel;
    }
    outcome
}
