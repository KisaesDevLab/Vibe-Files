//! Admin › Letter templates (ADDENDUM-tax-return-delivery.md §12.2, §15.6; T4, T5): the letters
//! a delivery sends to be signed, per return type. Each letter is a list of blocks: headings,
//! paragraphs (with `**bold**`, `*italic*` and `{Field}`), lists, simple tables and signature
//! blocks; any block can be "only for" some return types or packages. Saved in the cabinet
//! (`_Vibe/letters`), each save a new version; Preview lays it out for a sample client.

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_letters::{Block, FIELDS, Fields, ROLES, Template, role_words, store};

use crate::ui::{theme, widgets};

#[derive(Default)]
pub struct LetterTemplatesPage {
    loaded: bool,
    templates: Vec<Template>,
    /// The template being edited (a copy until Save).
    draft: Option<Template>,
    /// The block whose text Insert field adds to.
    last_block: usize,
    confirm_delete: bool,
    msg: Option<String>,
    err: Option<String>,
}

impl LetterTemplatesPage {
    pub fn reread(&mut self) {
        self.loaded = false;
    }
}

/// The text a block's editor shows: a list's items one per line, a table's rows one per line
/// with ` | ` between cells.
fn block_text(b: &Block) -> String {
    match b {
        Block::Heading { text, .. } | Block::Paragraph { text, .. } => text.clone(),
        Block::List { items, .. } => items.join("\n"),
        Block::Table { rows, .. } => rows
            .iter()
            .map(|r| r.join(" | "))
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Signature { .. } => String::new(),
    }
}

fn set_block_text(b: &mut Block, text: &str) {
    match b {
        Block::Heading { text: t, .. } | Block::Paragraph { text: t, .. } => *t = text.to_string(),
        Block::List { items, .. } => {
            *items = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        }
        Block::Table { rows, .. } => {
            *rows = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.split('|').map(|c| c.trim().to_string()).collect())
                .collect()
        }
        Block::Signature { .. } => {}
    }
}

fn kind_words(b: &Block) -> &'static str {
    match b {
        Block::Heading { .. } => "Heading",
        Block::Paragraph { .. } => "Paragraph",
        Block::List { .. } => "List (one item a line)",
        Block::Table { .. } => "Table (a row a line, cells split by |)",
        Block::Signature { .. } => "Signature",
    }
}

fn only_for_mut(b: &mut Block) -> &mut Vec<String> {
    match b {
        Block::Heading { only_for, .. }
        | Block::Paragraph { only_for, .. }
        | Block::List { only_for, .. }
        | Block::Table { only_for, .. }
        | Block::Signature { only_for, .. } => only_for,
    }
}

/// Fields for Preview: a joint 1040 for invented people.
pub fn sample_fields(firm: &str, year: i32, return_type: &str) -> Fields {
    let mut f = Fields::new();
    for (k, v) in [
        ("Client name", "Jordan and Casey Sample"),
        ("Client ID", "1001"),
        ("Client address", "100 Main Street, Springfield"),
        ("Signer first names", "Jordan and Casey"),
        ("Taxpayer name", "Jordan Sample"),
        ("Spouse name", "Casey Sample"),
        ("Signer name", "Jordan Sample"),
        ("Signer title", "President"),
        ("Covered returns", "Form 1040 and the state return"),
        ("Fee", "$1,265.68"),
        ("Fee options", "Tax Return Only, Advantage, Concierge"),
        ("Package name", "Advantage"),
        ("Package fee", "$1,392.68"),
        ("Preparer", "Pat Preparer"),
    ] {
        f.insert(k.into(), v.into());
    }
    f.insert("Firm name".into(), firm.into());
    f.insert("Tax year".into(), year.to_string());
    f.insert("Next year".into(), (year + 1).to_string());
    f.insert("Return type".into(), return_type.into());
    f.insert(
        "Date".into(),
        chrono::Local::now().format("%B %-d, %Y").to_string(),
    );
    f
}

/// Lays a template out for `fields` into a temporary PDF and opens it in the PDF viewer.
pub fn preview(t: &Template, fields: &Fields, return_type: &str) -> Result<(), String> {
    let laid = vf_letters::layout(t, fields, return_type, t.package.as_deref());
    let dir = std::env::temp_dir().join("vibe-files-letters");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = dir.join(format!(
        "{} ({}).pdf",
        vf_letters::store::safe_id(&t.id),
        return_type
    ));
    std::fs::write(&p, &laid.pdf).map_err(|e| format!("The preview could not be written: {e}"))?;
    crate::shell::run(&crate::shell::ShellAction::Open(p))
}

pub fn page(ui: &mut egui::Ui, page: &mut LetterTemplatesPage, env: &mut crate::ui::browse::Env) {
    ui.heading("Letter templates");
    if !page.loaded {
        page.loaded = true;
        page.err = store::seed_if_missing(env.root).err();
        match store::load_all(env.root) {
            Ok(t) => page.templates = t,
            Err(e) => page.err = Some(e),
        }
    }
    ui.label(widgets::caption(
        "The letters a tax return delivery sends to be signed on the portal: engagement letters, \
         consents and package addenda. Write **bold**, *italic* and {Field} in the text; mark a \
         block \"only for\" return types (1040, 1120, 1120-S, 1065) or packages. The seeded \
         samples say \"Sample — review before use\" until you save your own text.",
    ));
    if let Some(e) = &page.err {
        ui.colored_label(theme::error(), e);
    }
    if let Some(m) = &page.msg {
        ui.colored_label(theme::health_ok(), m);
    }
    ui.horizontal_top(|ui| {
        // The list.
        ui.vertical(|ui| {
            ui.set_width(260.0);
            if ui.button("New letter…").clicked() {
                page.draft = Some(Template {
                    id: format!("letter-{}", &vf_core::new_id()[..8]),
                    version: 0,
                    name: "New letter".into(),
                    return_types: vec![],
                    kind: "letter".into(),
                    package: None,
                    doc_type: String::new(),
                    letterhead: true,
                    default_ticked: false,
                    sample: false,
                    blocks: vec![Block::Paragraph {
                        text: String::new(),
                        only_for: vec![],
                    }],
                });
                page.msg = None;
            }
            ui.add_space(6.0);
            for t in &page.templates {
                let types = if t.return_types.is_empty() {
                    "every return".to_string()
                } else {
                    t.return_types.join(", ")
                };
                let label = format!(
                    "{}{}\n{types}{}",
                    t.name,
                    t.package
                        .as_ref()
                        .map(|p| format!(" ({p})"))
                        .unwrap_or_default(),
                    if t.sample { " · sample" } else { "" }
                );
                let chosen = page.draft.as_ref().is_some_and(|d| d.id == t.id);
                if ui.selectable_label(chosen, label).clicked() {
                    page.draft = Some(t.clone());
                    page.last_block = 0;
                    page.msg = None;
                    page.confirm_delete = false;
                }
            }
        });
        ui.separator();
        // The editor.
        ui.vertical(|ui| editor(ui, page, env));
    });
}

fn editor(ui: &mut egui::Ui, page: &mut LetterTemplatesPage, env: &mut crate::ui::browse::Env) {
    let Some(d) = &mut page.draft else {
        ui.label(widgets::caption(
            "Choose a letter on the left, or New letter….",
        ));
        return;
    };
    egui::Grid::new("letter-head")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name the client sees");
            widgets::named(
                ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(360.0)),
                "Letter name",
            );
            ui.end_row();
            ui.label("Return types");
            ui.horizontal(|ui| {
                for rt in vf_portal::delivery::RETURN_TYPES {
                    let mut on = d.return_types.iter().any(|x| x == rt);
                    if ui.checkbox(&mut on, rt).changed() {
                        d.return_types.retain(|x| x != rt);
                        if on {
                            d.return_types.push(rt.to_string());
                        }
                    }
                }
                ui.label(widgets::caption("(none ticked: every return)"));
            });
            ui.end_row();
            ui.label("Kind");
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.kind, "letter".to_string(), "Letter");
                ui.radio_value(&mut d.kind, "addendum".to_string(), "Package addendum for");
                let mut pkg = d.package.clone().unwrap_or_default();
                if ui
                    .add_enabled(
                        d.kind == "addendum",
                        egui::TextEdit::singleline(&mut pkg).desired_width(140.0),
                    )
                    .changed()
                {
                    d.package = Some(pkg).filter(|p| !p.trim().is_empty());
                }
            });
            ui.end_row();
            ui.label("Filed as (doc type)");
            ui.add(egui::TextEdit::singleline(&mut d.doc_type).desired_width(240.0));
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.letterhead, "On the firm's letterhead");
                ui.checkbox(&mut d.default_ticked, "Ticked in a new delivery");
            });
            ui.end_row();
        });
    ui.separator();
    // Insert field: into the block last edited.
    ui.horizontal(|ui| {
        ui.label("Insert field into the block last edited:");
        egui::ComboBox::from_id_salt("letter-field")
            .selected_text("Choose…")
            .show_ui(ui, |ui| {
                for f in FIELDS {
                    if ui.selectable_label(false, f).clicked()
                        && let Some(b) = d.blocks.get_mut(page.last_block)
                    {
                        let mut text = block_text(b);
                        text.push_str(&format!("{{{f}}}"));
                        set_block_text(b, &text);
                    }
                }
            });
    });
    let mut act: Option<(usize, i32)> = None;
    let n = d.blocks.len();
    for (i, b) in d.blocks.iter_mut().enumerate() {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(kind_words(b)).strong());
                if let Block::Signature { role, .. } = b {
                    egui::ComboBox::from_id_salt(("letter-role", i))
                        .selected_text(role_words(role))
                        .show_ui(ui, |ui| {
                            for r in ROLES {
                                ui.selectable_value(role, r.to_string(), role_words(r));
                            }
                        });
                }
                let mut only = only_for_mut(b).join(", ");
                ui.label("only for");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut only)
                            .desired_width(160.0)
                            .hint_text("every return"),
                    )
                    .changed()
                {
                    *only_for_mut(b) = only
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Remove").clicked() {
                        act = Some((i, 0));
                    }
                    if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).clicked() {
                        act = Some((i, 1));
                    }
                    if ui.add_enabled(i > 0, egui::Button::new("↑").small()).clicked() {
                        act = Some((i, -1));
                    }
                });
            });
            if !matches!(b, Block::Signature { .. }) {
                let mut text = block_text(b);
                let rows = if matches!(b, Block::Heading { .. }) { 1 } else { 3 };
                let r = ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(rows)
                        .desired_width(f32::INFINITY),
                );
                if r.changed() {
                    set_block_text(b, &text);
                }
                if r.has_focus() || r.changed() {
                    page.last_block = i;
                }
            } else {
                ui.label(widgets::caption(
                    "A signature line with the signer's name and a date (a spouse's is left out when there is none).",
                ));
            }
        });
    }
    match act {
        Some((i, 0)) => {
            d.blocks.remove(i);
        }
        Some((i, by)) => {
            let j = (i as i32 + by) as usize;
            d.blocks.swap(i, j);
        }
        None => {}
    }
    ui.horizontal(|ui| {
        ui.label("Add:");
        let empty = Vec::new;
        for (label, b) in [
            (
                "Heading",
                Block::Heading {
                    text: String::new(),
                    only_for: empty(),
                },
            ),
            (
                "Paragraph",
                Block::Paragraph {
                    text: String::new(),
                    only_for: empty(),
                },
            ),
            (
                "List",
                Block::List {
                    items: vec![],
                    only_for: empty(),
                },
            ),
            (
                "Table",
                Block::Table {
                    rows: vec![],
                    only_for: empty(),
                },
            ),
            (
                "Signature",
                Block::Signature {
                    role: "taxpayer".into(),
                    only_for: empty(),
                },
            ),
        ] {
            if ui.button(label).clicked() {
                d.blocks.push(b);
                page.last_block = d.blocks.len() - 1;
            }
        }
    });
    ui.separator();
    let mut save = false;
    let mut delete = false;
    ui.horizontal(|ui| {
        for rt in vf_portal::delivery::RETURN_TYPES
            .into_iter()
            .filter(|rt| d.offered_for(rt))
        {
            if ui.button(format!("Preview for a {rt}…")).clicked() {
                let firm = vf_fs::cabinet::read(env.root)
                    .ok()
                    .and_then(|c| c.name)
                    .unwrap_or_else(|| "Your firm".into());
                let year = chrono::Local::now()
                    .format("%Y")
                    .to_string()
                    .parse::<i32>()
                    .unwrap_or(2026)
                    - 1;
                if let Err(e) = preview(d, &sample_fields(&firm, year, rt), rt) {
                    page.err = Some(e);
                }
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(widgets::primary_button("Save")).clicked() {
                save = true;
            }
            if ui
                .button(RichText::new("Delete…").color(theme::error()))
                .clicked()
            {
                page.confirm_delete = true;
            }
        });
    });
    if page.confirm_delete {
        ui.horizontal(|ui| {
            ui.label("Delete this letter? Deliveries already sent keep their copy.");
            if ui.button("Delete").clicked() {
                delete = true;
            }
            if ui.button("Keep it").clicked() {
                page.confirm_delete = false;
            }
        });
    }
    let copy = d.clone();
    if save {
        // Saved by the firm: no longer the sample text.
        let mut t = copy.clone();
        t.sample = false;
        match store::save(env.root, &t) {
            Ok(saved) => {
                let unrecorded = crate::ui::portal::record(
                    env,
                    "letter_template_save",
                    vec![AuditEntry::new(
                        "letter_template_save",
                        None,
                        serde_json::json!({
                            "template_id": saved.id,
                            "version": saved.version,
                            "fields": ["name", "return_types", "kind", "doc_type", "blocks"],
                        }),
                    )],
                )
                .unwrap_or_default();
                page.msg = Some(format!("Saved as version {}.{unrecorded}", saved.version));
                page.err = None;
                page.draft = Some(saved);
                page.loaded = false;
            }
            Err(e) => page.err = Some(e),
        }
    }
    if delete {
        let id = copy.id.clone();
        match store::delete(env.root, &id) {
            Ok(()) => {
                let _ = crate::ui::portal::record(
                    env,
                    "letter_template_delete",
                    vec![AuditEntry::new(
                        "letter_template_delete",
                        None,
                        serde_json::json!({ "template_id": id }),
                    )],
                );
                page.msg = Some("Deleted.".into());
                page.draft = None;
                page.confirm_delete = false;
                page.loaded = false;
            }
            Err(e) => page.err = Some(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F2's gate: a 1040 joint letter and a 1065 letter laid out; each signature block sits on
    /// its printed line (checked on the rendered page), and nothing is printed where the
    /// signature goes.
    #[test]
    fn signature_blocks_land_on_the_printed_lines() {
        let Some(dir) = filer_pdf::pdf::find_pdfium(std::path::Path::new("")) else {
            panic!("pdfium is needed for this test");
        };
        let pdfium = filer_pdf::pdf::pdfium(&dir).unwrap();
        let t = tempfile::tempdir().unwrap();
        for (id, rt, roles) in [
            ("engagement-1040", "1040", vec!["taxpayer", "spouse"]),
            ("engagement-entity", "1065", vec!["officer"]),
        ] {
            let tpl = vf_letters::store::samples()
                .into_iter()
                .find(|x| x.id == id)
                .unwrap();
            let laid =
                vf_letters::layout(&tpl, &sample_fields("Sample CPA Firm", 2025, rt), rt, None);
            assert_eq!(
                laid.blocks
                    .iter()
                    .map(|b| b.role.as_str())
                    .collect::<Vec<_>>(),
                roles
            );
            let p = t.path().join(format!("{id}.pdf"));
            std::fs::write(&p, &laid.pdf).unwrap();
            for b in &laid.blocks {
                // One pixel a point.
                let img =
                    filer_pdf::pdf::render_page(pdfium, &p, (b.page - 1) as usize, 612).unwrap();
                let w = img.size[0];
                let dark = |x: usize, y: usize| img.pixels[y * w + x].r() < 140;
                let line = (792.0 - (b.y - 1.0)).round() as usize;
                let across = (b.x as usize + 10..(b.x + b.w) as usize - 10)
                    .filter(|&x| (line - 2..=line + 2).any(|y| dark(x, y)))
                    .count();
                assert!(
                    across as f32 > (b.w - 20.0) * 0.9,
                    "{id} {}: the line under the block ({across} dark of {})",
                    b.role,
                    b.w - 20.0
                );
                let (top, bottom) = (
                    (792.0 - (b.y + b.h - 2.0)) as usize,
                    (792.0 - (b.y + 4.0)) as usize,
                );
                let inside = (b.x as usize + 4..(b.x + b.w) as usize - 4)
                    .filter(|&x| (top..bottom).any(|y| dark(x, y)))
                    .count();
                assert_eq!(
                    inside, 0,
                    "{id} {}: printing inside the signature box",
                    b.role
                );
                let date_line = (792.0 - (b.date_y - 2.0)).round() as usize;
                assert!(
                    (b.date_x as usize + 5..(b.date_x + b.date_w) as usize - 5)
                        .all(|x| (date_line - 2..=date_line + 2).any(|y| dark(x, y))),
                    "{id} {}: the date line",
                    b.role
                );
            }
        }
    }

    #[test]
    fn lists_and_tables_edit_as_lines() {
        let mut b = Block::Table {
            rows: vec![],
            only_for: vec![],
        };
        set_block_text(&mut b, "Service | Fee\nForm 1040 | {Fee}\n");
        assert_eq!(block_text(&b), "Service | Fee\nForm 1040 | {Fee}");
        let mut l = Block::List {
            items: vec![],
            only_for: vec![],
        };
        set_block_text(&mut l, "One\n\n Two ");
        assert_eq!(block_text(&l), "One\nTwo");
    }
}
