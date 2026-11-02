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

pub enum PickerOutcome {
    Open,
    Cancel,
    Chosen(Client),
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
        if ui.button("Cancel").clicked() {
            outcome = PickerOutcome::Cancel;
        }
    });
    if matches!(outcome, PickerOutcome::Open) && modal.should_close() {
        outcome = PickerOutcome::Cancel;
    }
    outcome
}
