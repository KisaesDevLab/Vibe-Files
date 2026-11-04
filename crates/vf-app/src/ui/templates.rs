//! Admin › Folder templates (PA-24): the client folder tree new clients get, edited as a tree.
//! It is Filer Desktop's `template.json` in the cabinet's `_Vibe` (Filer's format and checks,
//! `filer_core::template`), so Intake's filing uses the same tree. Existing client folders are
//! not changed.

use egui::{RichText, TextEdit};
use filer_core::template::{self, TemplateNode};

use crate::ui::browse::Env;
use crate::ui::{theme, widgets};

#[derive(Default)]
pub struct TemplatesPage {
    /// The tree being edited (`None`: read it again).
    tree: Option<TemplateNode>,
    /// The tree as saved, to tell an edit.
    saved: Option<TemplateNode>,
    /// The selected node: child indexes from the root (empty = the client folder).
    selected: Vec<usize>,
    /// The Name field.
    pub(crate) name: String,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// Saved: Intake reloads (its filing uses the tree).
    pub changed: bool,
}

fn node_at<'a>(root: &'a mut TemplateNode, path: &[usize]) -> Option<&'a mut TemplateNode> {
    let mut n = root;
    for &i in path {
        n = n.children.get_mut(i)?;
    }
    Some(n)
}

fn rows(n: &TemplateNode, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, String)>) {
    out.push((path.clone(), n.name.clone()));
    for (i, c) in n.children.iter().enumerate() {
        path.push(i);
        rows(c, path, out);
        path.pop();
    }
}

pub fn show(ui: &mut egui::Ui, p: &mut TemplatesPage, env: &mut Env) {
    let dir = vf_fs::clients::master_dir(env.root);
    if p.tree.is_none() {
        let (tree, _, err) = template::load_or_seed(&dir);
        p.error = err;
        p.saved = Some(tree.clone());
        p.tree = Some(tree);
        p.selected.clear();
        p.name = p.tree.as_ref().map(|t| t.name.clone()).unwrap_or_default();
    }
    ui.label(RichText::new("Folder templates").strong().size(16.0));
    ui.label(widgets::caption(
        "The folders a new client gets. {Year} folders are made when something is filed for \
         that year. Kept in _Vibe\\template.json (Filer's format); existing client folders are \
         not changed.",
    ));
    ui.add_space(8.0);
    let tree = p.tree.as_mut().expect("loaded above");
    let mut list = Vec::new();
    rows(tree, &mut Vec::new(), &mut list);
    let mut pick = None;
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        egui::ScrollArea::vertical()
            .id_salt("tpl-tree")
            .max_height(420.0)
            .show(ui, |ui| {
                for (path, name) in &list {
                    let indent = path.len() as f32 * 18.0;
                    ui.horizontal(|ui| {
                        ui.add_space(indent);
                        let mut t = RichText::new(name);
                        if path.is_empty() {
                            t = t.strong();
                        }
                        if ui.selectable_label(*path == p.selected, t).clicked() {
                            pick = Some(path.clone());
                        }
                    });
                }
            });
        let ui = &mut cols[1];
        let is_root = p.selected.is_empty();
        ui.label(RichText::new(if is_root {
            "Client folder"
        } else {
            "Folder"
        })
        .strong());
        ui.horizontal(|ui| {
            let l = ui.label("Name");
            ui.add(TextEdit::singleline(&mut p.name).desired_width(220.0))
                .labelled_by(l.id);
            if ui.button("Rename").clicked()
                && let Some(n) = node_at(tree, &p.selected)
            {
                n.name = p.name.trim().to_string();
            }
        });
        if is_root {
            ui.label(widgets::caption(
                "Tokens: {Client}, {ID} and the client list's columns. A new name applies to new clients only.",
            ));
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button("Add sub-folder").clicked()
                && let Some(n) = node_at(tree, &p.selected)
            {
                n.children.push(TemplateNode::new("New folder"));
                let mut s = p.selected.clone();
                s.push(n.children.len() - 1);
                pick = Some(s);
            }
            if ui
                .button("Add {Year} folder")
                .on_hover_text("A folder per tax year inside this one (made when needed)")
                .clicked()
                && let Some(n) = node_at(tree, &p.selected)
            {
                n.children.push(TemplateNode::new("{Year}"));
            }
            if !is_root {
                let (parent, i) = p.selected.split_at(p.selected.len() - 1);
                let (parent, i) = (parent.to_vec(), i[0]);
                if ui.button("Move up").clicked()
                    && i > 0
                    && let Some(n) = node_at(tree, &parent)
                {
                    n.children.swap(i, i - 1);
                    let mut s = parent.clone();
                    s.push(i - 1);
                    pick = Some(s);
                }
                if ui.button("Move down").clicked()
                    && let Some(n) = node_at(tree, &parent)
                    && i + 1 < n.children.len()
                {
                    n.children.swap(i, i + 1);
                    let mut s = parent.clone();
                    s.push(i + 1);
                    pick = Some(s);
                }
                if ui
                    .button("Remove")
                    .on_hover_text("From the template only: folders on disk stay.")
                    .clicked()
                    && let Some(n) = node_at(tree, &parent)
                {
                    n.children.remove(i);
                    pick = Some(parent);
                }
            }
        });
    });
    if let Some(s) = pick {
        p.selected = s;
        p.name = node_at(tree, &p.selected)
            .map(|n| n.name.clone())
            .unwrap_or_default();
    }
    ui.add_space(8.0);
    let extra = filer_core::clients::load_cache(&dir)
        .ok()
        .flatten()
        .map(|l| filer_core::clients::extra_tokens(&l.column_map))
        .unwrap_or_default();
    let problems = template::validate_tree(tree, &extra);
    for e in &problems {
        ui.label(RichText::new(e).color(theme::error()));
    }
    let dirty = p.saved.as_ref() != Some(tree);
    let mut discard = false;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(dirty && problems.is_empty(), widgets::primary_button("Save template"))
            .clicked()
        {
            match template::save(&dir, tree) {
                Ok(()) => {
                    let audited = vf_core::tx::Transaction::new("template_edit")
                        .audit(vf_core::audit::AuditEntry::new(
                            "template_edit",
                            None,
                            serde_json::json!({ "folders": template::folder_templates(tree).len() }),
                        ))
                        .commit(
                            env.conn,
                            &mut vf_core::tx::NoFileOps,
                            &vf_core::tx::Actor::device(env.device_id),
                        );
                    p.saved = Some(tree.clone());
                    p.notice = Some("Template saved: new clients get these folders.".into());
                    p.error = audited
                        .err()
                        .map(|e| format!("Saved, but not recorded in the audit trail: {e}"));
                    p.changed = true;
                }
                Err(e) => p.error = Some(format!("Not saved: {e}")),
            }
        }
        if ui.add_enabled(dirty, egui::Button::new("Discard changes")).clicked() {
            discard = true;
        }
        if ui
            .button("Restore Filer's default")
            .on_hover_text("Put back the standard tree (save to keep it).")
            .clicked()
        {
            *tree = template::default_tree();
            p.selected.clear();
            p.name = tree.name.clone();
        }
    });
    if discard {
        p.tree = None;
    }
    if let Some(n) = &p.notice {
        ui.label(RichText::new(n).color(theme::ink_secondary()));
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(theme::error()));
    }
}
