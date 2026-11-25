//! Admin › Letter templates (ADDENDUM-tax-return-delivery.md §12.2, §15.6; T4, T5): the letters
//! a delivery sends to be signed, per return type. Each letter is a list of blocks: headings,
//! paragraphs, lists, simple tables and signature blocks; any block can be "only for" some
//! return types or packages. Saved in the cabinet (`_Vibe/letters`), each save a new version;
//! Preview lays it out for a sample client.
//!
//! The editor (owner 2026-11-24, Q24) draws the letter on a white page as it prints: headings
//! large, bold and italic shown as such, `{Field}`s as chips, lists with bullets, tables as a
//! grid of cells and signature blocks as dashed boxes, with a toolbar (Heading, B, I, List,
//! Table, Insert field, Insert signature). Bold and italic are kept in the text as `**` and `*`
//! (shown faint); the stored letter is the same block list as before.

use egui::text::{CCursor, CCursorRange, LayoutJob};
use egui::{Color32, FontFamily, FontId, Key, Modifiers, RichText, Stroke, TextFormat};
use vf_core::audit::AuditEntry;
use vf_letters::{Block, FIELDS, Fields, ROLES, Template, role_words, store};

use crate::ui::{theme, widgets};

/// Where the cursor is: a block, and in a list its item, in a table its row and cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct At {
    block: usize,
    row: usize,
    col: usize,
}

impl At {
    fn block(block: usize) -> Self {
        At {
            block,
            row: 0,
            col: 0,
        }
    }
}

#[derive(Default)]
pub struct LetterTemplatesPage {
    loaded: bool,
    templates: Vec<Template>,
    /// The template being edited (a copy until Save).
    draft: Option<Template>,
    /// The text the cursor was last in, and its selection (char indices): kept while a toolbar
    /// button has the focus, so B, I and Insert field act where the cursor was.
    at: Option<At>,
    sel: (usize, usize),
    /// A text to give the focus next frame, with the cursor at this char.
    focus_next: Option<(At, usize)>,
    preview_type: String,
    copy_to: String,
    /// The return types: the four built in and the firm's own (Q41).
    types: Vec<String>,
    /// Preview with a client (Q42): the client chosen (id, code, name) and the search text.
    preview_client: Option<(String, String, String)>,
    client_q: String,
    /// New return type…: open, its name, and whether the taxpayer and spouse sign.
    new_type: Option<(String, bool)>,
    confirm_delete: bool,
    msg: Option<String>,
    err: Option<String>,
}

impl LetterTemplatesPage {
    pub fn reread(&mut self) {
        self.loaded = false;
    }

    /// The blocks being edited (screens).
    #[cfg(test)]
    pub fn draft_blocks(&self) -> Vec<Block> {
        self.draft
            .as_ref()
            .map(|d| d.blocks.clone())
            .unwrap_or_default()
    }

    fn open(&mut self, t: Template) {
        self.draft = Some(t);
        self.at = None;
        self.sel = (0, 0);
        self.focus_next = None;
        self.confirm_delete = false;
    }
}

fn kind_words(b: &Block) -> &'static str {
    match b {
        Block::Heading { .. } => "Heading",
        Block::Paragraph { .. } => "Paragraph",
        Block::List { .. } => "List",
        Block::Table { .. } => "Table",
        Block::Signature { .. } => "Signature block",
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

/// The text at a place in a block (none in a signature block).
fn text_mut(b: &mut Block, at: At) -> Option<&mut String> {
    match b {
        Block::Heading { text, .. } | Block::Paragraph { text, .. } => Some(text),
        Block::List { items, .. } => items.get_mut(at.row),
        Block::Table { rows, .. } => rows.get_mut(at.row).and_then(|r| r.get_mut(at.col)),
        Block::Signature { .. } => None,
    }
}

/// The byte offset of a char index (the end when past it).
fn byte_at(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b)
}

/// `ins` put in place of the selection `a..b` (char indices); the cursor after it.
fn insert_text(text: &str, a: usize, b: usize, ins: &str) -> (String, usize) {
    let n = text.chars().count();
    let (a, b) = (a.min(b).min(n), a.max(b).min(n));
    let (ba, bb) = (byte_at(text, a), byte_at(text, b));
    (
        format!("{}{ins}{}", &text[..ba], &text[bb..]),
        a + ins.chars().count(),
    )
}

/// The selection `a..b` wrapped in `mark` (`**` bold, `*` italic). With nothing selected the
/// cursor lands between the two marks, ready to type.
fn wrap_text(text: &str, a: usize, b: usize, mark: &str) -> (String, usize) {
    let n = text.chars().count();
    let (a, b) = (a.min(b).min(n), a.max(b).min(n));
    let (ba, bb) = (byte_at(text, a), byte_at(text, b));
    let m = mark.chars().count();
    (
        format!(
            "{}{mark}{}{mark}{}",
            &text[..ba],
            &text[ba..bb],
            &text[bb..]
        ),
        if a == b { a + m } else { b + 2 * m },
    )
}

/// How a run of a block's text is drawn in the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    Text {
        bold: bool,
        italic: bool,
    },
    Mark,
    Field,
    /// A `[…]` fill-in the firm must replace before sending (A17).
    FillIn,
}

fn push_text(
    out: &mut Vec<(std::ops::Range<usize>, Look)>,
    s: usize,
    e: usize,
    bold: bool,
    italic: bool,
) {
    if e > s {
        out.push((s..e, Look::Text { bold, italic }));
    }
}

/// The text cut into runs: `**` and `*` marks (drawn faint), `{Field}`s (drawn as chips) and
/// text in its style, read as `vf_letters::spans` reads it. Every byte is in one run, so the
/// editor's cursor maps one to one onto the text.
fn runs(text: &str) -> Vec<(std::ops::Range<usize>, Look)> {
    let mut out = Vec::new();
    let (mut bold, mut italic) = (false, false);
    let (mut i, mut start) = (0, 0);
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with("**") {
            push_text(&mut out, start, i, bold, italic);
            out.push((i..i + 2, Look::Mark));
            bold = !bold;
            i += 2;
            start = i;
        } else if rest.starts_with('*') {
            push_text(&mut out, start, i, bold, italic);
            out.push((i..i + 1, Look::Mark));
            italic = !italic;
            i += 1;
            start = i;
        } else if let Some(after) = rest.strip_prefix('{')
            && let Some(j) = after
                .find('}')
                .filter(|&j| j > 0 && !after[..j].contains('{'))
        {
            push_text(&mut out, start, i, bold, italic);
            out.push((i..i + j + 2, Look::Field));
            i += j + 2;
            start = i;
        } else if let Some(after) = rest.strip_prefix('[')
            && let Some(j) = after.find(['[', ']']).filter(|&j| {
                after.as_bytes()[j] == b']'
                    && after[..j]
                        .chars()
                        .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            })
        {
            push_text(&mut out, start, i, bold, italic);
            out.push((i..i + j + 2, Look::FillIn));
            i += j + 2;
            start = i;
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    push_text(&mut out, start, text.len(), bold, italic);
    out
}

// The page is white in both themes, like the printed letter.
const INK: Color32 = Color32::from_rgb(28, 30, 34);
const FAINT: Color32 = Color32::from_rgb(160, 163, 170);
const CHIP_BG: Color32 = Color32::from_rgb(222, 233, 250);
const CHIP_INK: Color32 = Color32::from_rgb(28, 78, 150);
const FILL_BG: Color32 = Color32::from_rgb(255, 236, 150);
const BODY: f32 = 13.5;

/// Filer's semibold face, or the plain face where the fonts are not installed (a bare test
/// context).
fn semibold(ui: &egui::Ui) -> FontFamily {
    let f = FontFamily::Name(filer_intake::ui::theme::SEMIBOLD.into());
    if ui.fonts(|x| x.families().contains(&f)) {
        f
    } else {
        FontFamily::Proportional
    }
}

fn rich_job(ui: &egui::Ui, text: &str, size: f32, all_bold: bool, wrap: f32) -> LayoutJob {
    let bold_face = semibold(ui);
    let plain = |bold: bool, italic: bool| TextFormat {
        font_id: FontId::new(
            size,
            if bold || all_bold {
                bold_face.clone()
            } else {
                FontFamily::Proportional
            },
        ),
        color: INK,
        italics: italic,
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    for (r, look) in runs(text) {
        let f = match look {
            Look::Text { bold, italic } => plain(bold, italic),
            Look::Mark => TextFormat {
                font_id: FontId::proportional(size * 0.8),
                color: FAINT,
                ..Default::default()
            },
            Look::Field => TextFormat {
                font_id: FontId::proportional(size - 1.0),
                color: CHIP_INK,
                background: CHIP_BG,
                ..Default::default()
            },
            Look::FillIn => TextFormat {
                background: FILL_BG,
                ..plain(false, false)
            },
        };
        job.append(&text[r], 0.0, f);
    }
    if text.is_empty() {
        // An empty line keeps its height.
        job.append("", 0.0, plain(false, false));
    }
    job
}

/// One text on the page, drawn as it prints. Returns whether Enter (without Shift) was pressed
/// in it; Enter is then the caller's (a new paragraph or list item), not a line break.
#[allow(clippy::too_many_arguments)]
fn rich_edit(
    ui: &mut egui::Ui,
    page: &mut LetterTemplatesPage,
    id: egui::Id,
    at: At,
    text: &mut String,
    size: f32,
    bold: bool,
    width: f32,
    multiline: bool,
    hint: &str,
    label: &str,
) -> bool {
    // Given back a frame after the click on the toolbar (that click is "elsewhere" for the
    // text, which lets the focus go in the frame it happens).
    let clicking = ui.input(|i| i.pointer.any_pressed() || i.pointer.any_released());
    if clicking && page.focus_next.is_some_and(|(want, _)| want == at) {
        ui.ctx().request_repaint();
    } else if let Some((want, pos)) = page.focus_next
        && want == at
    {
        let mut st = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        st.cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(pos))));
        st.store(ui.ctx(), id);
        ui.memory_mut(|m| m.request_focus(id));
        page.focus_next = None;
    }
    let enter = ui.memory(|m| m.has_focus(id))
        && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
        ui.painter()
            .layout_job(rich_job(ui, buf.as_str(), size, bold, wrap))
    };
    let edit = if multiline {
        egui::TextEdit::multiline(text).desired_rows(1)
    } else {
        egui::TextEdit::singleline(text)
    };
    let out = edit
        .id(id)
        .frame(egui::Frame::NONE)
        .desired_width(width)
        .hint_text(RichText::new(hint).color(FAINT))
        .layouter(&mut layouter)
        .show(ui);
    let resp = out.response.response;
    if resp.has_focus() {
        page.at = Some(at);
        if let Some(c) = out.cursor_range {
            page.sel = (c.primary.index.into(), c.secondary.index.into());
        }
    }
    widgets::named(resp, label);
    enter
}

/// What a key or button on the page asks for, done after the page is drawn.
enum Op {
    /// A new empty paragraph at this index.
    Paragraph(usize),
    /// The paragraph split at the cursor (Enter).
    Split(usize),
    /// A new list item after this one, split at the cursor.
    Item(usize, usize),
    /// An empty list item removed (Backspace).
    DropItem(usize, usize),
    AddRow(usize),
    AddCol(usize),
    DelRow(usize),
    DelCol(usize),
}

fn apply(page: &mut LetterTemplatesPage, d: &mut Template, op: Op) {
    match op {
        Op::Paragraph(k) => {
            let k = k.min(d.blocks.len());
            d.blocks.insert(
                k,
                Block::Paragraph {
                    text: String::new(),
                    only_for: vec![],
                },
            );
            page.focus_next = Some((At::block(k), 0));
        }
        Op::Split(i) => {
            if let Some(Block::Paragraph { text, only_for }) = d.blocks.get_mut(i) {
                let b = byte_at(text, page.sel.0.max(page.sel.1));
                let tail = text[b..].to_string();
                text.truncate(b);
                let only_for = only_for.clone();
                d.blocks.insert(
                    i + 1,
                    Block::Paragraph {
                        text: tail,
                        only_for,
                    },
                );
                page.focus_next = Some((At::block(i + 1), 0));
            }
        }
        Op::Item(i, j) => {
            if let Some(Block::List { items, .. }) = d.blocks.get_mut(i)
                && j < items.len()
            {
                let b = byte_at(&items[j], page.sel.0.max(page.sel.1));
                let tail = items[j][b..].to_string();
                items[j].truncate(b);
                items.insert(j + 1, tail);
                page.focus_next = Some((
                    At {
                        block: i,
                        row: j + 1,
                        col: 0,
                    },
                    0,
                ));
            }
        }
        Op::DropItem(i, j) => {
            if let Some(Block::List { items, only_for }) = d.blocks.get_mut(i) {
                if items.len() > 1 {
                    items.remove(j);
                    let r = j.saturating_sub(1);
                    let end = items[r].chars().count();
                    page.focus_next = Some((
                        At {
                            block: i,
                            row: r,
                            col: 0,
                        },
                        if j == 0 { 0 } else { end },
                    ));
                } else {
                    // The last item gone: the list becomes a paragraph.
                    let only_for = std::mem::take(only_for);
                    d.blocks[i] = Block::Paragraph {
                        text: String::new(),
                        only_for,
                    };
                    page.focus_next = Some((At::block(i), 0));
                }
            }
        }
        Op::AddRow(i) | Op::AddCol(i) | Op::DelRow(i) | Op::DelCol(i) => {
            let at = page.at.filter(|a| a.block == i).unwrap_or(At::block(i));
            if let Some(Block::Table { rows, .. }) = d.blocks.get_mut(i) {
                let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                match op {
                    Op::AddRow(_) => {
                        rows.insert((at.row + 1).min(rows.len()), vec![String::new(); cols])
                    }
                    Op::AddCol(_) => {
                        for r in rows.iter_mut() {
                            r.insert((at.col + 1).min(r.len()), String::new());
                        }
                    }
                    Op::DelRow(_) if rows.len() > 1 => {
                        rows.remove(at.row.min(rows.len() - 1));
                    }
                    Op::DelCol(_) if cols > 1 => {
                        for r in rows.iter_mut() {
                            if at.col < r.len() {
                                r.remove(at.col);
                            }
                        }
                    }
                    _ => {}
                }
                page.at = Some(At::block(i));
                page.sel = (0, 0);
            }
        }
    }
}

/// Where a new block goes: after the one the cursor is in, else at the end.
fn after_cursor(page: &LetterTemplatesPage, d: &Template) -> usize {
    page.at
        .map_or(d.blocks.len(), |a| (a.block + 1).min(d.blocks.len()))
}

fn insert_block(page: &mut LetterTemplatesPage, d: &mut Template, b: Block) {
    let k = after_cursor(page, d);
    let has_text = !matches!(b, Block::Signature { .. });
    d.blocks.insert(k, b);
    page.at = Some(At::block(k));
    page.sel = (0, 0);
    if has_text {
        page.focus_next = Some((At::block(k), 0));
    }
}

/// B and I: the selection wrapped in `**` or `*`.
fn mark(page: &mut LetterTemplatesPage, d: &mut Template, m: &str) {
    let Some(at) = page.at else {
        page.err = Some("Click in the text first, then press B or I.".into());
        return;
    };
    let Some(text) = d.blocks.get_mut(at.block).and_then(|b| text_mut(b, at)) else {
        return;
    };
    let (t, cur) = wrap_text(text, page.sel.0, page.sel.1, m);
    *text = t;
    page.sel = (cur, cur);
    page.focus_next = Some((at, cur));
}

/// Insert field: `{Field}` at the cursor (in place of a selection); with no text chosen, into
/// a new paragraph at the end.
fn insert_field(page: &mut LetterTemplatesPage, d: &mut Template, field: &str) {
    let at = match page.at.filter(|a| {
        d.blocks
            .get(a.block)
            .is_some_and(|b| !matches!(b, Block::Signature { .. }))
    }) {
        Some(a) => a,
        None => {
            d.blocks.push(Block::Paragraph {
                text: String::new(),
                only_for: vec![],
            });
            page.sel = (0, 0);
            At::block(d.blocks.len() - 1)
        }
    };
    if let Some(text) = text_mut(&mut d.blocks[at.block], at) {
        let (t, cur) = insert_text(text, page.sel.0, page.sel.1, &format!("{{{field}}}"));
        *text = t;
        page.at = Some(at);
        page.sel = (cur, cur);
        page.focus_next = Some((at, cur));
    }
}

/// Heading: the paragraph the cursor is in becomes a heading, a heading a paragraph; elsewhere
/// a new heading.
fn toggle_heading(page: &mut LetterTemplatesPage, d: &mut Template) {
    if let Some(at) = page.at
        && let Some(b) = d.blocks.get_mut(at.block)
    {
        match b {
            Block::Paragraph { text, only_for } => {
                *b = Block::Heading {
                    text: std::mem::take(text),
                    only_for: std::mem::take(only_for),
                };
                page.focus_next = Some((At::block(at.block), page.sel.0));
                return;
            }
            Block::Heading { text, only_for } => {
                *b = Block::Paragraph {
                    text: std::mem::take(text),
                    only_for: std::mem::take(only_for),
                };
                page.focus_next = Some((At::block(at.block), page.sel.0));
                return;
            }
            _ => {}
        }
    }
    insert_block(
        page,
        d,
        Block::Heading {
            text: String::new(),
            only_for: vec![],
        },
    );
}

/// List: the paragraph becomes a list (a line an item), a list a paragraph; elsewhere a new
/// list.
fn toggle_list(page: &mut LetterTemplatesPage, d: &mut Template) {
    if let Some(at) = page.at
        && let Some(b) = d.blocks.get_mut(at.block)
    {
        match b {
            Block::Paragraph { text, only_for } => {
                let mut items: Vec<String> = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect();
                if items.is_empty() {
                    items.push(String::new());
                }
                *b = Block::List {
                    items,
                    only_for: std::mem::take(only_for),
                };
                page.focus_next = Some((At::block(at.block), 0));
                return;
            }
            Block::List { items, only_for } => {
                *b = Block::Paragraph {
                    text: items.join(" "),
                    only_for: std::mem::take(only_for),
                };
                page.focus_next = Some((At::block(at.block), 0));
                return;
            }
            _ => {}
        }
    }
    insert_block(
        page,
        d,
        Block::List {
            items: vec![String::new()],
            only_for: vec![],
        },
    );
}

/// Fields for Preview: a joint 1040 for invented people.
pub fn sample_fields(firm: &str, year: i32, return_type: &str) -> Fields {
    let mut f = Fields::new();
    for (k, v) in [
        ("Client name", "Jordan and Casey Sample"),
        ("Client ID", "1001"),
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
        ("Partner name", "Morgan Example"),
        ("Partner title", "Partner"),
        ("Services", "Form 1040, Bookkeeping"),
        (
            "Fee lines",
            "Form 1040: $1,265.68
Bookkeeping: $450.00 a month",
        ),
        ("One-time fee", "$1,265.68"),
        ("Monthly fee", "$450.00"),
    ] {
        f.insert(k.into(), v.into());
    }
    f.insert("Firm name".into(), firm.into());
    f.insert("Tax year".into(), year.to_string());
    f.insert("Next year".into(), (year + 1).to_string());
    f.insert("Agreement year".into(), (year + 1).to_string());
    f.insert("Return type".into(), return_type.into());
    f.insert(
        "Date".into(),
        chrono::Local::now().format("%B %-d, %Y").to_string(),
    );
    f
}

/// A client's own values for Preview (Q42): their name and ID, the signers from their Portal
/// contacts (the taxpayer and spouse for a return signed so, else the contact as the signer),
/// the tax year, the return type, the firm and the date. The fee fields have no value outside a
/// delivery, so they show as written.
pub fn client_fields(
    root: &std::path::Path,
    client: &(String, String, String),
    year: i32,
    return_type: &str,
) -> Fields {
    let (id, code, name) = client;
    let row = vf_fs::contacts::record(root, id)
        .map(|r| r.effective())
        .unwrap_or_default();
    let full = |a: &str, b: &str| format!("{} {}", a.trim(), b.trim()).trim().to_string();
    let taxpayer = full(&row.first, &row.last);
    let spouse = full(&row.spouse_first, &row.spouse_last);
    let individual = vf_portal::delivery::is_individual(root, return_type);
    let firsts: Vec<&str> = [row.first.trim(), row.spouse_first.trim()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .take(if individual { 2 } else { 1 })
        .collect();
    let firm = vf_fs::cabinet::read(root)
        .ok()
        .and_then(|c| c.name)
        .unwrap_or_default();
    let mut f = Fields::new();
    for (k, v) in [
        ("Client name", name.clone()),
        ("Client ID", code.clone()),
        ("Signer first names", firsts.join(" and ")),
        (
            "Taxpayer name",
            if individual {
                taxpayer.clone()
            } else {
                String::new()
            },
        ),
        (
            "Spouse name",
            if individual { spouse } else { String::new() },
        ),
        (
            "Signer name",
            if individual { String::new() } else { taxpayer },
        ),
        ("Tax year", year.to_string()),
        ("Next year", (year + 1).to_string()),
        ("Return type", return_type.to_string()),
        ("Firm name", firm),
        (
            "Date",
            chrono::Local::now().format("%B %-d, %Y").to_string(),
        ),
    ] {
        if !v.is_empty() {
            f.insert(k.into(), v);
        }
    }
    f
}

/// Clients whose name or ID holds `q` (at most 8), for Preview with a client.
fn find_clients(conn: &vf_core::db::Connection, q: &str) -> Vec<(String, String, String)> {
    let like = format!("%{}%", q.trim().replace(['%', '_'], ""));
    let Ok(mut st) = conn.prepare(
        "SELECT id, ext_id, name FROM client WHERE deleted_at IS NULL
         AND (name LIKE ?1 OR ext_id LIKE ?1) ORDER BY name LIMIT 8",
    ) else {
        return Vec::new();
    };
    st.query_map([like], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Where letter previews are written: `<data>\cache\letters` (owner 2026-11-24, Q27), set
/// when the app starts; emptied then and when the app closes (they hold client names).
static PREVIEW_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

pub fn set_preview_dir(data: &std::path::Path) {
    let dir = data.join("cache").join("letters");
    clear_previews_in(&dir);
    let _ = PREVIEW_DIR.set(dir);
}

pub fn preview_dir() -> std::path::PathBuf {
    PREVIEW_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("vibe-files-letters"))
}

fn clear_previews_in(dir: &std::path::Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "pdf") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Empties the previews folder (the app is closing).
pub fn clear_previews() {
    if let Some(d) = PREVIEW_DIR.get() {
        clear_previews_in(d);
    }
}

/// Lays a template out for `fields` into a temporary PDF and opens it in the PDF viewer:
/// `context` is the return type with the signer class (`"1040|Individual"`).
pub fn preview(t: &Template, fields: &Fields, context: &str) -> Result<(), String> {
    let return_type = context.split('|').next().unwrap_or(context);
    let laid = vf_letters::layout(t, fields, context, t.package.as_deref());
    let dir = preview_dir();
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
        page.types = vf_portal::delivery::service_names(env.root);
        match store::load_all(env.root) {
            Ok(t) => page.templates = t,
            Err(e) => page.err = Some(e),
        }
    }
    ui.label(widgets::caption(
        "The letters a tax return delivery sends to be signed on the portal: engagement letters, \
         consents and package addenda. Click in the page to write; the toolbar adds headings, \
         bold, italic, lists, tables, fields and signature blocks. The seeded samples say \
         \"Sample — review before use\" until you save your own text.",
    ));
    if let Some(e) = &page.err {
        ui.colored_label(theme::error(), e);
    }
    if let Some(m) = &page.msg {
        ui.colored_label(theme::health_ok(), m);
    }
    let draft = page.draft.take();
    let (left, right) = (210.0, 290.0);
    let centre = (ui.available_width() - left - right - 64.0).max(420.0);
    let mut next: Option<Option<Template>> = None;
    let mut d = draft;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(left);
            if let Some(t) = letter_list(ui, page, d.as_ref().map(|d| d.id.as_str())) {
                page.msg = None;
                next = Some(Some(t));
            }
            new_return_type(ui, page, env);
        });
        ui.separator();
        let Some(d) = d.as_mut() else {
            ui.label(widgets::caption(
                "Choose a letter on the left, or New letter….",
            ));
            return;
        };
        ui.vertical(|ui| {
            ui.set_width(centre);
            toolbar(ui, page, d);
            egui::ScrollArea::vertical()
                .id_salt("letter-page")
                .max_height(640.0)
                .show(ui, |ui| paper(ui, page, d));
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.set_width(right);
            if let Some(x) = side(ui, page, d, env) {
                next = Some(x);
            }
        });
    });
    match next {
        Some(Some(t)) => page.open(t),
        Some(None) => page.draft = None,
        None => page.draft = d,
    }
}

/// The letters, grouped by the return types they are for. Returns a letter chosen (or a new
/// one).
fn letter_list(
    ui: &mut egui::Ui,
    page: &LetterTemplatesPage,
    open: Option<&str>,
) -> Option<Template> {
    let mut pick = None;
    if ui.button("New letter…").clicked() {
        pick = Some(Template {
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
    }
    ui.add_space(6.0);
    let groups = std::iter::once(None).chain(page.types.iter().map(|t| Some(t.as_str())));
    for rt in groups {
        let these: Vec<&Template> = page
            .templates
            .iter()
            .filter(|t| match rt {
                None => t.return_types.is_empty(),
                Some(rt) => t.return_types.iter().any(|x| x == rt),
            })
            .collect();
        if these.is_empty() {
            continue;
        }
        ui.label(
            RichText::new(rt.unwrap_or("Every return"))
                .small()
                .strong()
                .color(theme::ink_muted()),
        );
        for t in these {
            let label = format!(
                "{}{}{}",
                t.name,
                t.package
                    .as_ref()
                    .map(|p| format!(" ({p})"))
                    .unwrap_or_default(),
                if t.sample { " · sample" } else { "" }
            );
            if ui
                .selectable_label(open == Some(t.id.as_str()), label)
                .clicked()
            {
                pick = Some(t.clone());
            }
        }
        ui.add_space(4.0);
    }
    pick
}

/// New return type… (Q41): a return type of the firm's own (990, 1041, CA 540…), for every
/// PC; letters, pricing plans and deliveries then offer it.
fn new_return_type(
    ui: &mut egui::Ui,
    page: &mut LetterTemplatesPage,
    env: &mut crate::ui::browse::Env,
) {
    ui.add_space(6.0);
    let Some((name, individual)) = &mut page.new_type else {
        if ui.button("New return type…").clicked() {
            page.new_type = Some((String::new(), false));
        }
        return;
    };
    ui.label(RichText::new("New return type").strong());
    widgets::named(
        ui.add(
            egui::TextEdit::singleline(name)
                .desired_width(180.0)
                .hint_text("e.g. 990 or CA 540"),
        ),
        "Return type name",
    );
    ui.radio_value(individual, true, "Signed by the taxpayer and spouse");
    ui.radio_value(individual, false, "Signed by an officer or partner");
    let (mut add, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        add = ui.add(widgets::primary_button("Add")).clicked();
        cancel = ui.button("Cancel").clicked();
    });
    if cancel {
        page.new_type = None;
    } else if add {
        let (n, ind) = (name.clone(), *individual);
        match vf_portal::delivery::add_return_type(env.root, &n, ind) {
            Ok(t) => {
                let unrecorded = crate::ui::portal::record(
                    env,
                    "return_type_add",
                    vec![AuditEntry::new(
                        "return_type_add",
                        None,
                        serde_json::json!({ "name": t.name, "individual": t.individual }),
                    )],
                )
                .unwrap_or_default();
                page.types = vf_portal::delivery::service_names(env.root);
                page.msg = Some(format!(
                    "Added the {} return type: tick it on a letter to offer the letter for it.{unrecorded}",
                    t.name
                ));
                page.err = None;
                page.new_type = None;
            }
            Err(e) => page.err = Some(e),
        }
    }
}

fn toolbar(ui: &mut egui::Ui, page: &mut LetterTemplatesPage, d: &mut Template) {
    ui.horizontal(|ui| {
        if ui
            .button("Heading")
            .on_hover_text("The paragraph the cursor is in becomes a heading, or back")
            .clicked()
        {
            toggle_heading(page, d);
        }
        if ui
            .button(RichText::new("B").strong())
            .on_hover_text("Bold")
            .clicked()
        {
            mark(page, d, "**");
        }
        if ui
            .button(RichText::new("I").italics())
            .on_hover_text("Italic")
            .clicked()
        {
            mark(page, d, "*");
        }
        if ui
            .button("List")
            .on_hover_text("The paragraph becomes a list (a line an item), or back")
            .clicked()
        {
            toggle_list(page, d);
        }
        if ui
            .button("Table")
            .on_hover_text("A table after the cursor")
            .clicked()
        {
            insert_block(
                page,
                d,
                Block::Table {
                    rows: vec![vec![String::new(); 2]; 2],
                    only_for: vec![],
                },
            );
        }
        ui.menu_button("Insert field", |ui| {
            for &f in FIELDS {
                if ui.button(f).clicked() {
                    insert_field(page, d, f);
                    ui.close();
                }
            }
        });
        ui.menu_button("Insert signature", |ui| {
            for r in ROLES {
                if ui.button(role_words(r)).clicked() {
                    insert_block(
                        page,
                        d,
                        Block::Signature {
                            role: r.to_string(),
                            only_for: vec![],
                        },
                    );
                    ui.close();
                }
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let stored = page.templates.iter().find(|t| t.id == d.id);
            let (words, color) = match stored {
                Some(s) if s == d => (format!("Saved · version {}", s.version), theme::ink_muted()),
                Some(_) => ("Not saved".to_string(), theme::error()),
                None => ("Not saved yet".to_string(), theme::error()),
            };
            ui.label(RichText::new(words).small().color(color));
        });
    });
}

/// The letter on a white page, each block drawn as it prints and edited in place.
fn paper(ui: &mut egui::Ui, page: &mut LetterTemplatesPage, d: &mut Template) {
    let inner = (ui.available_width() - 84.0).max(300.0);
    let mut op: Option<Op> = None;
    let tid = d.id.clone();
    let id = |at: At| egui::Id::unique(("letter-text", tid.as_str(), at.block, at.row, at.col));
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, Color32::from_gray(205)))
        .inner_margin(egui::Margin::symmetric(40, 30))
        .show(ui, |ui| {
            ui.set_width(inner);
            {
                let v = ui.visuals_mut();
                v.text_cursor.stroke.color = INK;
                v.selection.bg_fill = Color32::from_rgb(188, 212, 248);
                v.selection.stroke.color = INK;
                v.override_text_color = Some(INK);
                v.extreme_bg_color = Color32::WHITE;
                for w in [&mut v.widgets.inactive, &mut v.widgets.hovered] {
                    w.weak_bg_fill = Color32::from_gray(236);
                    w.bg_fill = Color32::from_gray(236);
                    w.fg_stroke.color = INK;
                }
            }
            if d.letterhead {
                ui.label(
                    RichText::new("The firm's letterhead prints here")
                        .small()
                        .color(FAINT),
                );
                ui.add_space(6.0);
            }
            for i in 0..d.blocks.len() {
                let selected = page.at.is_some_and(|a| a.block == i);
                let only = d.blocks[i].only_for().to_vec();
                let r = ui
                    .scope(|ui| {
                        if !only.is_empty() {
                            ui.label(
                                RichText::new(format!("Only for {}", only.join(", ")))
                                    .small()
                                    .color(FAINT),
                            );
                        }
                        match &mut d.blocks[i] {
                            Block::Heading { text, .. } => {
                                let at = At::block(i);
                                if rich_edit(
                                    ui,
                                    page,
                                    id(at),
                                    at,
                                    text,
                                    18.0,
                                    true,
                                    inner,
                                    false,
                                    "Heading",
                                    "Letter heading",
                                ) {
                                    op = Some(Op::Paragraph(i + 1));
                                }
                            }
                            Block::Paragraph { text, .. } => {
                                let at = At::block(i);
                                if rich_edit(
                                    ui,
                                    page,
                                    id(at),
                                    at,
                                    text,
                                    BODY,
                                    false,
                                    inner,
                                    true,
                                    "Type here…",
                                    "Letter paragraph",
                                ) {
                                    op = Some(Op::Split(i));
                                }
                            }
                            Block::List { items, .. } => {
                                if items.is_empty() {
                                    items.push(String::new());
                                }
                                for (j, item) in items.iter_mut().enumerate() {
                                    let at = At {
                                        block: i,
                                        row: j,
                                        col: 0,
                                    };
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("•").size(BODY).color(INK));
                                        let back = item.is_empty()
                                            && ui.memory(|m| m.has_focus(id(at)))
                                            && ui.input(|x| x.key_pressed(Key::Backspace));
                                        if rich_edit(
                                            ui,
                                            page,
                                            id(at),
                                            at,
                                            item,
                                            BODY,
                                            false,
                                            inner - 24.0,
                                            false,
                                            "List item",
                                            "Letter list item",
                                        ) {
                                            op = Some(Op::Item(i, j));
                                        } else if back {
                                            op = Some(Op::DropItem(i, j));
                                        }
                                    });
                                }
                            }
                            Block::Table { rows, .. } => {
                                if rows.is_empty() {
                                    *rows = vec![vec![String::new(); 2]; 2];
                                }
                                let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                                for r in rows.iter_mut() {
                                    r.resize(cols, String::new());
                                }
                                let cw = ((inner - 4.0) / cols as f32 - 10.0).max(60.0);
                                egui::Grid::new(("letter-table", tid.as_str(), i))
                                    .spacing([0.0, 0.0])
                                    .min_col_width(cw + 10.0)
                                    .show(ui, |ui| {
                                        for (r, row) in rows.iter_mut().enumerate() {
                                            for (c, cell) in row.iter_mut().enumerate() {
                                                let at = At {
                                                    block: i,
                                                    row: r,
                                                    col: c,
                                                };
                                                egui::Frame::new()
                                                    .stroke(Stroke::new(
                                                        1.0,
                                                        Color32::from_gray(190),
                                                    ))
                                                    .inner_margin(egui::Margin::same(4))
                                                    .show(ui, |ui| {
                                                        rich_edit(
                                                            ui,
                                                            page,
                                                            id(at),
                                                            at,
                                                            cell,
                                                            BODY - 1.0,
                                                            r == 0,
                                                            cw,
                                                            false,
                                                            "",
                                                            "Letter table cell",
                                                        );
                                                    });
                                            }
                                            ui.end_row();
                                        }
                                    });
                                if selected {
                                    ui.horizontal(|ui| {
                                        if ui.small_button("Add row").clicked() {
                                            op = Some(Op::AddRow(i));
                                        }
                                        if ui.small_button("Add column").clicked() {
                                            op = Some(Op::AddCol(i));
                                        }
                                        if ui.small_button("Remove row").clicked() {
                                            op = Some(Op::DelRow(i));
                                        }
                                        if ui.small_button("Remove column").clicked() {
                                            op = Some(Op::DelCol(i));
                                        }
                                    });
                                }
                            }
                            Block::Signature { role, .. } => {
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(300.0, 62.0),
                                    egui::Sense::click(),
                                );
                                let stroke = Stroke::new(
                                    1.2,
                                    if selected {
                                        theme::accent()
                                    } else {
                                        Color32::from_gray(150)
                                    },
                                );
                                let pts = [
                                    rect.left_top(),
                                    rect.right_top(),
                                    rect.right_bottom(),
                                    rect.left_bottom(),
                                    rect.left_top(),
                                ];
                                ui.painter()
                                    .extend(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0));
                                ui.painter().text(
                                    rect.left_top() + egui::vec2(10.0, 9.0),
                                    egui::Align2::LEFT_TOP,
                                    format!("Signature: {}", role_words(role)),
                                    FontId::proportional(BODY - 1.0),
                                    INK,
                                );
                                ui.painter().text(
                                    rect.left_bottom() + egui::vec2(10.0, -9.0),
                                    egui::Align2::LEFT_BOTTOM,
                                    "Signed on the portal, with the name and date",
                                    FontId::proportional(11.0),
                                    FAINT,
                                );
                                if widgets::named(resp, "Signature block").clicked() {
                                    page.at = Some(At::block(i));
                                    page.sel = (0, 0);
                                }
                            }
                        }
                    })
                    .response;
                if selected {
                    let x = r.rect.left() - 14.0;
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(x, r.rect.top()),
                            egui::pos2(x + 3.0, r.rect.bottom()),
                        ),
                        1.0,
                        theme::accent(),
                    );
                }
                ui.add_space(8.0);
            }
            if ui
                .add(
                    egui::Label::new(RichText::new("+ Paragraph").small().color(FAINT))
                        .sense(egui::Sense::click()),
                )
                .clicked()
            {
                op = Some(Op::Paragraph(d.blocks.len()));
            }
        });
    if let Some(op) = op {
        apply(page, d, op);
    }
}

/// The right pane: the letter's settings, the block the cursor is in, the fields, Preview,
/// Copy, Save and Delete. Returns a letter to open instead (`Some(None)`: none, deleted).
fn side(
    ui: &mut egui::Ui,
    page: &mut LetterTemplatesPage,
    d: &mut Template,
    env: &mut crate::ui::browse::Env,
) -> Option<Option<Template>> {
    let types = page.types.clone();
    let is_type = |x: &str| types.iter().any(|t| t == x);
    ui.label(RichText::new("This letter").strong());
    ui.label("Name the client sees");
    widgets::named(
        ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY)),
        "Letter name",
    );
    ui.label("Return types (none ticked: every return)");
    ui.horizontal_wrapped(|ui| {
        for rt in &types {
            let mut on = d.return_types.iter().any(|x| x == rt);
            if ui.checkbox(&mut on, rt.as_str()).changed() {
                d.return_types.retain(|x| x != rt);
                if on {
                    d.return_types.push(rt.to_string());
                }
            }
        }
    });
    ui.horizontal(|ui| {
        ui.radio_value(&mut d.kind, "letter".to_string(), "Letter");
        ui.radio_value(&mut d.kind, "addendum".to_string(), "Package addendum");
    });
    ui.horizontal(|ui| {
        ui.radio_value(&mut d.kind, vf_letters::TERMS.to_string(), "Terms and Conditions")
            .on_hover_text("Its text goes where a letter says {Terms and Conditions}, covered by the letter's signature.");
        ui.radio_value(&mut d.kind, vf_letters::CLOSURE.to_string(), "Closure letter")
            .on_hover_text("Sent by End engagement…: read on the portal, not signed.");
    });
    let left = d.all_fill_ins();
    if !left.is_empty() {
        ui.colored_label(
            theme::warn(),
            format!("Fill in before sending: {}", left.join(", ")),
        );
    }
    if d.kind == "addendum" {
        ui.horizontal(|ui| {
            ui.label("for the package");
            let mut pkg = d.package.clone().unwrap_or_default();
            if ui
                .add(egui::TextEdit::singleline(&mut pkg).desired_width(140.0))
                .changed()
            {
                d.package = Some(pkg).filter(|p| !p.trim().is_empty());
            }
        });
    }
    ui.horizontal(|ui| {
        ui.label("Filed as");
        ui.add(
            egui::TextEdit::singleline(&mut d.doc_type)
                .desired_width(f32::INFINITY)
                .hint_text("doc type"),
        );
    });
    ui.checkbox(&mut d.letterhead, "On the firm's letterhead");
    ui.checkbox(&mut d.default_ticked, "Ticked in a new delivery");
    ui.separator();
    // The block the cursor is in.
    let n = d.blocks.len();
    if let Some(at) = page.at.filter(|a| a.block < n) {
        let i = at.block;
        ui.label(RichText::new(format!("This block: {}", kind_words(&d.blocks[i]))).strong());
        if let Block::Signature { role, .. } = &mut d.blocks[i] {
            egui::ComboBox::from_id_salt("letter-role")
                .selected_text(role_words(role))
                .show_ui(ui, |ui| {
                    for r in ROLES {
                        ui.selectable_value(role, r.to_string(), role_words(r));
                    }
                });
        }
        ui.label("Only for (none ticked: every return)");
        let only = only_for_mut(&mut d.blocks[i]);
        ui.horizontal_wrapped(|ui| {
            for rt in &types {
                let mut on = only.iter().any(|x| x == rt);
                if ui.checkbox(&mut on, rt.as_str()).changed() {
                    only.retain(|x| x != rt);
                    if on {
                        only.push(rt.to_string());
                    }
                }
            }
        });
        let mut pkgs = only
            .iter()
            .filter(|x| !is_type(x))
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        ui.horizontal(|ui| {
            ui.label("or packages");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut pkgs)
                        .desired_width(f32::INFINITY)
                        .hint_text("Advantage, Concierge"),
                )
                .changed()
            {
                only.retain(|x| is_type(x));
                only.extend(
                    pkgs.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from),
                );
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(i > 0, egui::Button::new("Move up"))
                .clicked()
            {
                d.blocks.swap(i, i - 1);
                page.at = Some(At::block(i - 1));
            }
            if ui
                .add_enabled(i + 1 < n, egui::Button::new("Move down"))
                .clicked()
            {
                d.blocks.swap(i, i + 1);
                page.at = Some(At::block(i + 1));
            }
            if ui
                .button(RichText::new("Remove").color(theme::error()))
                .clicked()
            {
                d.blocks.remove(i);
                page.at = None;
                page.sel = (0, 0);
            }
        });
    } else {
        ui.label(widgets::caption(
            "Click a block on the page to set who it is for, move it or remove it.",
        ));
    }
    ui.separator();
    ui.label(RichText::new("Fields").strong());
    ui.label(widgets::caption("Click one to put it where the cursor is."));
    ui.horizontal_wrapped(|ui| {
        for &f in FIELDS {
            if ui.small_button(f).clicked() {
                insert_field(page, d, f);
            }
        }
    });
    ui.separator();
    // Preview, Copy, Save, Delete.
    let offered: Vec<&str> = types
        .iter()
        .map(String::as_str)
        .filter(|rt| d.offered_for(rt))
        .collect();
    if !offered.contains(&page.preview_type.as_str()) {
        page.preview_type = offered.first().copied().unwrap_or("1040").to_string();
    }
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("letter-preview-type")
            .selected_text(format!("as a {}", page.preview_type))
            .show_ui(ui, |ui| {
                for rt in &offered {
                    ui.selectable_value(&mut page.preview_type, rt.to_string(), *rt);
                }
            });
        if ui.button("Preview…").clicked() {
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
            let rt = page.preview_type.clone();
            let fields = match &page.preview_client {
                Some(c) => client_fields(env.root, c, year, &rt),
                None => sample_fields(&firm, year, &rt),
            };
            let all = vf_letters::store::load_all(env.root).unwrap_or_default();
            let ctx = vf_portal::delivery::letter_context(env.root, &rt);
            if let Err(e) = preview(&vf_letters::store::as_sent(d, &all), &fields, &ctx) {
                page.err = Some(e);
            }
        }
    });
    // With: invented people, or a client of the cabinet (Q42).
    ui.horizontal(|ui| {
        ui.label("with");
        let shown = page
            .preview_client
            .as_ref()
            .map(|c| format!("{} ({})", c.2, c.1))
            .unwrap_or_else(|| "invented people".into());
        ui.label(RichText::new(shown).strong());
        if page.preview_client.is_some() && ui.small_button("Invented people").clicked() {
            page.preview_client = None;
        }
    });
    widgets::named(
        ui.add(
            egui::TextEdit::singleline(&mut page.client_q)
                .desired_width(f32::INFINITY)
                .hint_text("Find a client to preview with"),
        ),
        "Preview client",
    );
    if page.client_q.trim().chars().count() >= 2 {
        for c in find_clients(env.conn, &page.client_q) {
            if ui
                .selectable_label(false, format!("{} ({})", c.2, c.1))
                .clicked()
            {
                page.preview_client = Some(c);
                page.client_q.clear();
            }
        }
    }
    let mut open: Option<Option<Template>> = None;
    ui.horizontal(|ui| {
        if page.copy_to.is_empty() {
            page.copy_to = "1120-S".into();
        }
        egui::ComboBox::from_id_salt("letter-copy-to")
            .selected_text(&page.copy_to)
            .show_ui(ui, |ui| {
                for rt in &types {
                    ui.selectable_value(&mut page.copy_to, rt.clone(), rt.as_str());
                }
            });
        if ui
            .button("Copy to this return type")
            .on_hover_text("A new letter with this one's text, for one return type")
            .clicked()
        {
            let mut c = d.clone();
            c.id = format!("letter-{}", &vf_core::new_id()[..8]);
            c.version = 0;
            c.sample = false;
            c.return_types = vec![page.copy_to.clone()];
            page.msg = Some(format!(
                "Copied for the {}: press Save to keep it.",
                page.copy_to
            ));
            open = Some(Some(c));
        }
    });
    ui.add_space(6.0);
    let mut save = false;
    let mut delete = false;
    ui.horizontal(|ui| {
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
    if page.confirm_delete {
        ui.label("Delete this letter? Deliveries already sent keep their copy.");
        ui.horizontal(|ui| {
            if ui.button("Delete").clicked() {
                delete = true;
            }
            if ui.button("Keep it").clicked() {
                page.confirm_delete = false;
            }
        });
    }
    if save {
        // Saved by the firm: no longer the sample text. Empty list items and table rows are
        // left out.
        let mut t = d.clone();
        t.sample = false;
        for b in &mut t.blocks {
            match b {
                Block::List { items, .. } => items.retain(|x| !x.trim().is_empty()),
                Block::Table { rows, .. } => {
                    rows.retain(|r| r.iter().any(|c| !c.trim().is_empty()))
                }
                _ => {}
            }
        }
        t.blocks.retain(|b| match b {
            Block::List { items, .. } => !items.is_empty(),
            Block::Table { rows, .. } => !rows.is_empty(),
            _ => true,
        });
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
                page.err = None;
                page.loaded = false;
                // Shown as saved at once (the list is read again next frame).
                page.templates.retain(|x| x.id != saved.id);
                page.templates.push(saved.clone());
                *d = saved.clone();
                page.msg = Some(format!("Saved as version {}.{unrecorded}", saved.version));
            }
            Err(e) => page.err = Some(e),
        }
    }
    if delete {
        let id = d.id.clone();
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
                page.confirm_delete = false;
                page.loaded = false;
                open = Some(None);
            }
            Err(e) => page.err = Some(e),
        }
    }
    open
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

    /// The editor's runs cover every byte of the text (so the cursor maps one to one), read
    /// bold, italic and fields as the layout does, and keep non-ASCII text whole.
    #[test]
    fn runs_cover_the_text() {
        let text = "We **will** prepare *your* {Tax year} return — {Fee} {not a field";
        let rs = runs(text);
        let mut at = 0;
        for (r, _) in &rs {
            assert_eq!(r.start, at);
            at = r.end;
        }
        assert_eq!(at, text.len());
        let look = |s: &str| rs.iter().find(|(r, _)| &text[r.clone()] == s).map(|x| x.1);
        assert_eq!(
            look("will"),
            Some(Look::Text {
                bold: true,
                italic: false
            })
        );
        assert_eq!(
            look("your"),
            Some(Look::Text {
                bold: false,
                italic: true
            })
        );
        assert_eq!(look("{Tax year}"), Some(Look::Field));
        assert_eq!(look("**"), Some(Look::Mark));
        assert!(
            rs.iter()
                .all(|(r, l)| *l != Look::Field || !text[r.clone()].contains("not"))
        );
    }

    /// B, I and Insert field act on char positions, also past non-ASCII text.
    #[test]
    fn marks_and_fields_at_the_cursor() {
        let t = "Fee — due now";
        // "due" is chars 6..9.
        assert_eq!(wrap_text(t, 9, 6, "**"), ("Fee — **due** now".into(), 13));
        assert_eq!(wrap_text(t, 6, 6, "*").1, 7);
        assert_eq!(
            insert_text(t, 4, 5, "{Fee}"),
            ("Fee {Fee} due now".into(), 9)
        );
        assert_eq!(insert_text("", 3, 3, "{Date}"), ("{Date}".into(), 6));
    }

    /// Enter splits the paragraph at the cursor; Backspace in an empty last list item turns
    /// the list into a paragraph.
    #[test]
    fn enter_and_backspace() {
        let mut page = LetterTemplatesPage::default();
        let mut d = vf_letters::store::samples().remove(0);
        d.blocks = vec![
            Block::Paragraph {
                text: "Première ligne. Seconde.".into(),
                only_for: vec!["1040".into()],
            },
            Block::List {
                items: vec![String::new()],
                only_for: vec![],
            },
        ];
        page.sel = (15, 15);
        apply(&mut page, &mut d, Op::Split(0));
        assert_eq!(
            d.blocks[..2],
            [
                Block::Paragraph {
                    text: "Première ligne.".into(),
                    only_for: vec!["1040".into()]
                },
                Block::Paragraph {
                    text: " Seconde.".into(),
                    only_for: vec!["1040".into()]
                }
            ]
        );
        assert_eq!(page.focus_next, Some((At::block(1), 0)));
        apply(&mut page, &mut d, Op::DropItem(2, 0));
        assert!(matches!(d.blocks[2], Block::Paragraph { .. }));
        page.at = Some(At::block(0));
        toggle_heading(&mut page, &mut d);
        assert!(matches!(d.blocks[0], Block::Heading { .. }));
    }
}
