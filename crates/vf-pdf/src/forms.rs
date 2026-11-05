//! Form filling (PA-27): a PDF's fillable fields (AcroForm) read with their place on the page,
//! and filled in. Each filled widget gets a new appearance stream drawn here (Helvetica), so
//! every viewer, Print and Flatten show the typed value; check boxes and radio buttons switch
//! to their own "on" appearance. The form stays fillable.
//!
//! Push buttons, signature fields and file-select fields are not offered. Values are written as
//! PDF text strings (UTF-16 when not plain ASCII); the drawn appearance uses WinAnsi, so a
//! character outside it shows as `?` on the page while the field keeps the real text.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat, dictionary};

/// What kind of field, with what it needs to be filled.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text {
        multiline: bool,
        /// Characters allowed (`/MaxLen`).
        max_len: Option<usize>,
        /// One character per box (`/MaxLen` cells).
        comb: bool,
        password: bool,
    },
    /// A check box: its value is `on` or `Off`.
    Check { on: String },
    /// Radio buttons: the value is one of `options` or `Off`.
    Radio { options: Vec<String> },
    /// A drop-down (`combo`) or a list.
    Choice {
        options: Vec<ChoiceOption>,
        combo: bool,
        /// A drop-down that also takes typed text.
        editable: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    /// What is stored.
    pub value: String,
    /// What is shown.
    pub label: String,
}

/// Where a field shows on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    /// 0-based page.
    pub page: usize,
    /// User space `[x0, y0, x1, y1]`.
    pub rect: [f32; 4],
    /// A check box or radio button's "on" state name.
    pub on: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// Fully qualified name (`parent.child`): the key for [`fill`].
    pub name: String,
    /// The field's tooltip (`/TU`), the friendlier name when there is one.
    pub label: Option<String>,
    pub kind: FieldKind,
    /// Text or choice: the text; check box and radio: the state name or `Off`.
    pub value: String,
    pub read_only: bool,
    pub widgets: Vec<Widget>,
}

impl Field {
    /// What to call it on screen.
    pub fn title(&self) -> &str {
        self.label
            .as_deref()
            .filter(|l| !l.trim().is_empty())
            .unwrap_or(&self.name)
    }

    /// The first page it shows on.
    pub fn page(&self) -> usize {
        self.widgets.first().map_or(0, |w| w.page)
    }

    /// For a check box or radio button: whether `value` turns it on.
    pub fn is_on(value: &str) -> bool {
        !value.is_empty() && value != "Off"
    }
}

/// The fillable fields of the PDF at `path`, in page order then top to bottom.
pub fn read(path: &Path) -> Result<Vec<Field>, String> {
    let doc = Document::load(path).map_err(|e| format!("cannot read the PDF: {e}"))?;
    if doc.is_encrypted() {
        return Err("the PDF is password protected".into());
    }
    Ok(read_doc(&doc))
}

/// [`read`] on a loaded document.
pub fn read_doc(doc: &Document) -> Vec<Field> {
    let mut out: Vec<Field> = found(doc).into_iter().map(|f| f.field).collect();
    out.sort_by(|a, b| {
        let key = |f: &Field| {
            f.widgets
                .first()
                .map_or((usize::MAX, 0.0, 0.0), |w| (w.page, -w.rect[3], w.rect[0]))
        };
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0)
            .then(ka.1.total_cmp(&kb.1))
            .then(ka.2.total_cmp(&kb.2))
    });
    out
}

/// Fills `values` (field name → value, as [`Field::value`]) into `src`, writing `out`.
/// Returns how many fields changed. A name not in the form, or a read-only field, is an error:
/// nothing is written.
pub fn fill(src: &Path, out: &Path, values: &[(String, String)]) -> Result<usize, String> {
    let mut doc = Document::load(src).map_err(|e| format!("cannot read the PDF: {e}"))?;
    if doc.is_encrypted() {
        return Err("the PDF is password protected".into());
    }
    let n = fill_doc(&mut doc, values)?;
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)
        .map_err(|e| format!("cannot write the PDF: {e}"))?;
    std::fs::write(out, bytes).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(n)
}

/// [`fill`] on a loaded document.
pub fn fill_doc(doc: &mut Document, values: &[(String, String)]) -> Result<usize, String> {
    let fields = found(doc);
    // Two field dictionaries may carry the same full name (they are one field to a reader).
    let mut by_name: HashMap<&str, Vec<&Found>> = HashMap::new();
    for f in &fields {
        by_name.entry(f.field.name.as_str()).or_default().push(f);
    }
    let mut changed = 0;
    for (name, value) in values {
        let all = by_name
            .get(name.as_str())
            .ok_or_else(|| format!("the form has no field \"{name}\""))?;
        if let Some(f) = all.iter().find(|f| f.field.read_only) {
            return Err(format!("\"{}\" is read-only", f.field.title()));
        }
        if all.iter().all(|f| f.field.value == *value) {
            continue;
        }
        for f in all {
            set_value(doc, f, value)?;
        }
        changed += 1;
    }
    if changed > 0 {
        drop_xfa(doc);
    }
    Ok(changed)
}

/// Removes the form's XFA data (`/AcroForm /XFA`). Acrobat fills an XFA form's fields from
/// the XFA data, not from `/V`, so a form filled here would show its old values there; without
/// it, Acrobat uses the AcroForm fields like every other viewer. (What other fillers do.)
fn drop_xfa(doc: &mut Document) {
    let form = match doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok()) {
        Some(Object::Reference(id)) => Some(*id),
        _ => None,
    };
    match form {
        Some(id) => {
            if let Ok(d) = doc.get_dictionary_mut(id) {
                d.remove(b"XFA");
            }
        }
        None => {
            if let Ok(Object::Dictionary(d)) =
                doc.catalog_mut().and_then(|c| c.get_mut(b"AcroForm"))
            {
                d.remove(b"XFA");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Reading

/// A field with where it lives in the file.
struct Found {
    field: Field,
    /// The field dictionary (`/V` goes here).
    id: ObjectId,
    /// Its widget annotations (one may be the field itself), with their page.
    widgets: Vec<ObjectId>,
    da: Option<String>,
    /// `/Q`: 0 left, 1 centred, 2 right.
    quad: i64,
}

/// What a field takes from its parents.
#[derive(Clone, Default)]
struct Inherit {
    ft: Option<Vec<u8>>,
    ff: i64,
    v: Option<Object>,
    da: Option<String>,
    q: i64,
    opt: Option<Object>,
    max_len: Option<i64>,
}

fn found(doc: &Document) -> Vec<Found> {
    let Some(form) = acroform(doc) else {
        return vec![];
    };
    let default_da = form.get(b"DA").ok().and_then(|o| text_of(doc, o));
    let fields = match form.get(b"Fields").map(|o| deref(doc, o)) {
        Ok(Some(Object::Array(a))) => a.clone(),
        _ => return vec![],
    };
    let pages = widget_pages(doc);
    let base = Inherit {
        da: default_da,
        ..Inherit::default()
    };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for f in fields {
        if let Object::Reference(id) = f {
            walk(doc, id, "", &base, &pages, &mut seen, &mut out, 0);
        }
    }
    out
}

fn acroform(doc: &Document) -> Option<&Dictionary> {
    let cat = doc.catalog().ok()?;
    deref(doc, cat.get(b"AcroForm").ok()?)?.as_dict().ok()
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Object> {
    doc.dereference(o).ok().map(|(_, o)| o)
}

/// Widget annotation → its page (from each page's `/Annots`).
fn widget_pages(doc: &Document) -> HashMap<ObjectId, usize> {
    let mut map = HashMap::new();
    for (i, (_, page)) in doc.get_pages().into_iter().enumerate() {
        let Ok(p) = doc.get_dictionary(page) else {
            continue;
        };
        let Some(Object::Array(annots)) = p.get(b"Annots").ok().and_then(|o| deref(doc, o)) else {
            continue;
        };
        for a in annots {
            if let Object::Reference(id) = a {
                map.insert(*id, i);
            }
        }
    }
    map
}

#[allow(clippy::too_many_arguments)]
fn walk(
    doc: &Document,
    id: ObjectId,
    parent: &str,
    inherited: &Inherit,
    pages: &HashMap<ObjectId, usize>,
    seen: &mut HashSet<ObjectId>,
    out: &mut Vec<Found>,
    depth: usize,
) {
    if depth > 32 || !seen.insert(id) {
        return;
    }
    let Ok(d) = doc.get_dictionary(id) else {
        return;
    };
    let mut inh = inherited.clone();
    if let Ok(o) = d.get(b"FT")
        && let Ok(n) = o.as_name()
    {
        inh.ft = Some(n.to_vec());
    }
    if let Some(ff) = d.get(b"Ff").ok().and_then(|o| o.as_i64().ok()) {
        inh.ff = ff;
    }
    if let Ok(v) = d.get(b"V") {
        inh.v = deref(doc, v).cloned();
    }
    if let Some(da) = d.get(b"DA").ok().and_then(|o| text_of(doc, o)) {
        inh.da = Some(da);
    }
    if let Some(q) = d.get(b"Q").ok().and_then(|o| o.as_i64().ok()) {
        inh.q = q;
    }
    if let Ok(o) = d.get(b"Opt") {
        inh.opt = deref(doc, o).cloned();
    }
    if let Some(m) = d.get(b"MaxLen").ok().and_then(|o| o.as_i64().ok()) {
        inh.max_len = Some(m);
    }
    let partial = d.get(b"T").ok().and_then(|o| text_of(doc, o));
    let name = match (&partial, parent.is_empty()) {
        (Some(t), true) => t.clone(),
        (Some(t), false) => format!("{parent}.{t}"),
        (None, _) => parent.to_string(),
    };
    let kids: Vec<ObjectId> = match d.get(b"Kids").ok().and_then(|o| deref(doc, o)) {
        Some(Object::Array(a)) => a.iter().filter_map(|k| k.as_reference().ok()).collect(),
        _ => vec![],
    };
    let field_kids: Vec<ObjectId> = kids
        .iter()
        .copied()
        .filter(|k| {
            // A field has /T or /Kids; a widget has a /Rect. A nameless grouping dict is a field.
            doc.get_dictionary(*k)
                .is_ok_and(|kd| kd.has(b"T") || kd.has(b"Kids") || !kd.has(b"Rect"))
        })
        .collect();
    if !field_kids.is_empty() {
        for k in field_kids {
            walk(doc, k, &name, &inh, pages, seen, out, depth + 1);
        }
        return;
    }
    // A terminal field: its widgets are its kids, or itself.
    let widget_ids: Vec<ObjectId> = if kids.is_empty() { vec![id] } else { kids };
    let label = d.get(b"TU").ok().and_then(|o| text_of(doc, o));
    let mut widgets = Vec::new();
    for w in &widget_ids {
        let Ok(wd) = doc.get_dictionary(*w) else {
            continue;
        };
        let (Some(page), Some(rect)) = (pages.get(w), rect_of(wd)) else {
            continue; // not on a page
        };
        widgets.push(Widget {
            page: *page,
            rect,
            on: on_state(doc, wd),
        });
    }
    if widgets.is_empty() || name.is_empty() {
        return;
    }
    let Some(kind) = kind_of(doc, &inh, &widgets) else {
        return;
    };
    let value = match &kind {
        FieldKind::Check { .. } | FieldKind::Radio { .. } => {
            match inh.v.as_ref().and_then(|v| v.as_name().ok()) {
                Some(n) => name_text(n),
                None => "Off".into(),
            }
        }
        _ => match &inh.v {
            Some(Object::Array(a)) => a.first().and_then(|o| text_of(doc, o)).unwrap_or_default(),
            Some(o) => text_of(doc, o).unwrap_or_default(),
            None => String::new(),
        },
    };
    out.push(Found {
        field: Field {
            name,
            label,
            kind,
            value,
            read_only: inh.ff & 1 != 0,
            widgets,
        },
        id,
        widgets: widget_ids,
        da: inh.da.clone(),
        quad: inh.q,
    });
}

fn kind_of(doc: &Document, inh: &Inherit, widgets: &[Widget]) -> Option<FieldKind> {
    let ff = inh.ff;
    match inh.ft.as_deref()? {
        b"Tx" => {
            if ff & (1 << 20) != 0 {
                return None; // file select
            }
            Some(FieldKind::Text {
                multiline: ff & (1 << 12) != 0,
                max_len: inh.max_len.and_then(|m| usize::try_from(m).ok()),
                comb: ff & (1 << 24) != 0
                    && ff & (1 << 13) == 0
                    && inh.max_len.is_some_and(|m| m > 0),
                password: ff & (1 << 13) != 0,
            })
        }
        b"Btn" => {
            if ff & (1 << 16) != 0 {
                return None; // push button
            }
            let mut states: Vec<String> = Vec::new();
            for w in widgets {
                if let Some(on) = &w.on
                    && !states.contains(on)
                {
                    states.push(on.clone());
                }
            }
            // Radio buttons, or check boxes whose boxes have different values (one of them on at
            // a time, as tax forms use them): each box turns on with its own value.
            if ff & (1 << 15) != 0 || states.len() > 1 {
                Some(FieldKind::Radio { options: states })
            } else {
                Some(FieldKind::Check {
                    on: states.into_iter().next().unwrap_or_else(|| "Yes".into()),
                })
            }
        }
        b"Ch" => {
            let options = match inh.opt.as_ref() {
                Some(Object::Array(a)) => a
                    .iter()
                    .filter_map(|o| match deref(doc, o)? {
                        Object::Array(pair) if pair.len() >= 2 => Some(ChoiceOption {
                            value: text_of(doc, &pair[0])?,
                            label: text_of(doc, &pair[1])?,
                        }),
                        o => {
                            let t = text_of(doc, o)?;
                            Some(ChoiceOption {
                                value: t.clone(),
                                label: t,
                            })
                        }
                    })
                    .collect(),
                _ => vec![],
            };
            Some(FieldKind::Choice {
                options,
                combo: ff & (1 << 17) != 0,
                editable: ff & (1 << 18) != 0,
            })
        }
        _ => None, // signatures and unknown kinds
    }
}

fn rect_of(d: &Dictionary) -> Option<[f32; 4]> {
    let v: Vec<f32> = d
        .get(b"Rect")
        .ok()?
        .as_array()
        .ok()?
        .iter()
        .filter_map(|o| o.as_float().ok())
        .collect();
    (v.len() == 4).then(|| {
        [
            v[0].min(v[2]),
            v[1].min(v[3]),
            v[0].max(v[2]),
            v[1].max(v[3]),
        ]
    })
}

/// A check box or radio widget's "on" appearance name (any `/AP /N` state but `Off`).
fn on_state(doc: &Document, w: &Dictionary) -> Option<String> {
    let ap = deref(doc, w.get(b"AP").ok()?)?.as_dict().ok()?;
    let n = deref(doc, ap.get(b"N").ok()?)?.as_dict().ok()?;
    n.iter()
        .map(|(k, _)| k)
        .find(|k| k.as_slice() != b"Off")
        .map(|k| name_text(k))
}

/// A PDF name as text: UTF-8 when it is, else each byte as its Latin-1 character (so a state
/// name like `S\xEDn` round-trips; [`state_bytes`] gives the widget's own bytes back).
fn name_text(n: &[u8]) -> String {
    match std::str::from_utf8(n) {
        Ok(t) => t.to_string(),
        Err(_) => n.iter().map(|&b| b as char).collect(),
    }
}

/// The bytes of appearance state `state` as a widget of `f` names it.
fn state_bytes(doc: &Document, f: &Found, state: &str) -> Vec<u8> {
    for w in &f.widgets {
        let Some(n) = doc
            .get_dictionary(*w)
            .ok()
            .and_then(|wd| deref(doc, wd.get(b"AP").ok()?))
            .and_then(|ap| ap.as_dict().ok())
            .and_then(|ap| deref(doc, ap.get(b"N").ok()?))
            .and_then(|n| n.as_dict().ok())
        else {
            continue;
        };
        if let Some((k, _)) = n.iter().find(|(k, _)| name_text(k) == state) {
            return k.clone();
        }
    }
    state.as_bytes().to_vec()
}

/// A PDF text string (or name) as text: UTF-16BE with its BOM, else PDFDocEncoding (read as
/// Latin-1, which it matches for the printable range).
fn text_of(doc: &Document, o: &Object) -> Option<String> {
    match deref(doc, o)? {
        Object::String(b, _) => Some(decode_text(b)),
        Object::Name(n) => Some(name_text(n)),
        _ => None,
    }
}

fn decode_text(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let units: Vec<u16> = b[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
    } else if b.len() >= 3 && b[..3] == [0xEF, 0xBB, 0xBF] {
        String::from_utf8_lossy(&b[3..]).into_owned()
    } else {
        b.iter().map(|&c| c as char).collect()
    }
}

fn encode_text(s: &str) -> Object {
    if s.chars()
        .all(|c| (' '..='~').contains(&c) || c == '\n' || c == '\r')
    {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(b, StringFormat::Hexadecimal)
    }
}

// ---------------------------------------------------------------------------------------------
// Filling

fn set_value(doc: &mut Document, f: &Found, value: &str) -> Result<(), String> {
    match &f.field.kind {
        FieldKind::Check { .. } | FieldKind::Radio { .. } => {
            let state = if Field::is_on(value) { value } else { "Off" };
            if let FieldKind::Radio { options } = &f.field.kind
                && Field::is_on(state)
                && !options.iter().any(|o| o == state)
            {
                return Err(format!("\"{}\" has no choice \"{state}\"", f.field.title()));
            }
            let raw = state_bytes(doc, f, state);
            set(doc, f.id, "V", Object::Name(raw.clone()))?;
            for w in &f.widgets {
                let has = doc
                    .get_dictionary(*w)
                    .ok()
                    .and_then(|wd| on_state(doc, wd))
                    .is_some_and(|on| on == state);
                let as_ = if has { raw.clone() } else { b"Off".to_vec() };
                set(doc, *w, "AS", Object::Name(as_))?;
            }
        }
        FieldKind::Text { .. } | FieldKind::Choice { .. } => {
            if let FieldKind::Text {
                max_len: Some(m), ..
            } = &f.field.kind
                && value.chars().count() > *m
            {
                return Err(format!(
                    "\"{}\" takes at most {m} characters",
                    f.field.title()
                ));
            }
            set(doc, f.id, "V", encode_text(value))?;
            if let Ok(d) = doc.get_dictionary_mut(f.id) {
                d.remove(b"RV");
            }
            if let FieldKind::Choice { options, .. } = &f.field.kind {
                // `/I`, the chosen options' indices, is what Acrobat highlights in a list.
                let d = doc.get_dictionary_mut(f.id).map_err(|e| e.to_string())?;
                match options.iter().position(|o| o.value == value) {
                    Some(i) => d.set("I", vec![Object::Integer(i as i64)]),
                    None => {
                        d.remove(b"I");
                    }
                }
            }
            for w in &f.widgets {
                draw_widget(doc, f, *w, value)?;
            }
        }
    }
    Ok(())
}

/// A new normal appearance for text or choice widget `w` of `f`, showing `value`. A widget
/// turned by `/MK /R` gets its text turned with it.
fn draw_widget(doc: &mut Document, f: &Found, w: ObjectId, value: &str) -> Result<(), String> {
    let Ok(wd) = doc.get_dictionary(w) else {
        return Ok(());
    };
    let Some(rect) = rect_of(wd) else {
        return Ok(());
    };
    let look = Look::of(doc, wd, f.da.as_deref());
    let turn = wd
        .get(b"MK")
        .ok()
        .and_then(|o| deref(doc, o))
        .and_then(|o| o.as_dict().ok())
        .and_then(|mk| mk.get(b"R").ok())
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0)
        .rem_euclid(360);
    let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
    // Drawn upright in the widget's own frame, then turned (§12.5.5 maps it onto /Rect).
    let (w_, h_, matrix): (f32, f32, [f32; 6]) = match turn {
        90 => (rh, rw, [0.0, 1.0, -1.0, 0.0, 0.0, 0.0]),
        180 => (rw, rh, [-1.0, 0.0, 0.0, -1.0, 0.0, 0.0]),
        270 => (rh, rw, [0.0, -1.0, 1.0, 0.0, 0.0, 0.0]),
        _ => (rw, rh, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
    };
    let content = appearance(&f.field.kind, value, [0.0, 0.0, w_, h_], &look, f.quad);
    let stream = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), w_.into(), h_.into()],
            "Matrix" => matrix.iter().map(|&x| Object::Real(x)).collect::<Vec<_>>(),
            "Resources" => dictionary! {
                "Font" => dictionary! {
                    FONT => dictionary! {
                        "Type" => "Font",
                        "Subtype" => "Type1",
                        "BaseFont" => "Helvetica",
                        "Encoding" => "WinAnsiEncoding",
                    },
                },
            },
        },
        content.into_bytes(),
    );
    let ap = doc.add_object(stream);
    set(doc, w, "AP", Object::Dictionary(dictionary! { "N" => ap }))
}

/// Draws the appearance of every text and choice field that has a value but no appearance, or
/// of all of them when the form asks viewers to (`/NeedAppearances true`). Forms filled by other
/// programs are often left like that; Flatten calls this so their values are not lost.
/// Returns how many widgets were drawn.
pub fn draw_missing_appearances(doc: &mut Document) -> usize {
    let need_all = acroform(doc)
        .and_then(|f| f.get(b"NeedAppearances").ok())
        .and_then(|o| o.as_bool().ok())
        .unwrap_or(false);
    let mut n = 0;
    for f in found(doc) {
        if !matches!(
            f.field.kind,
            FieldKind::Text { .. } | FieldKind::Choice { .. }
        ) || f.field.value.is_empty()
        {
            continue;
        }
        for w in &f.widgets {
            let has_ap = doc
                .get_dictionary(*w)
                .ok()
                .and_then(|wd| deref(doc, wd.get(b"AP").ok()?))
                .and_then(|ap| ap.as_dict().ok())
                .is_some_and(|ap| ap.has(b"N"));
            if (need_all || !has_ap) && draw_widget(doc, &f, *w, &f.field.value.clone()).is_ok() {
                n += 1;
            }
        }
    }
    n
}

fn set(doc: &mut Document, id: ObjectId, key: &str, v: Object) -> Result<(), String> {
    doc.get_dictionary_mut(id)
        .map_err(|e| e.to_string())?
        .set(key, v);
    Ok(())
}

/// The font resource name in the appearances drawn here.
const FONT: &str = "VfHelv";

/// How a widget is drawn: from its `/DA` and `/MK`.
struct Look {
    /// 0: fit the box.
    size: f32,
    color: [f32; 3],
    background: Option<[f32; 3]>,
    border: Option<[f32; 3]>,
}

impl Look {
    fn of(doc: &Document, w: &Dictionary, field_da: Option<&str>) -> Look {
        let da = w
            .get(b"DA")
            .ok()
            .and_then(|o| text_of(doc, o))
            .or_else(|| field_da.map(str::to_string))
            .unwrap_or_default();
        let toks: Vec<&str> = da.split_whitespace().collect();
        let num = |i: usize| {
            toks.get(i)
                .and_then(|t| t.parse::<f32>().ok())
                .filter(|x| x.is_finite())
        };
        let mut size = 0.0;
        let mut color = [0.0; 3];
        for (i, t) in toks.iter().enumerate() {
            match *t {
                "Tf" if i >= 1 => size = num(i - 1).unwrap_or(0.0).min(200.0),
                "g" if i >= 1 => {
                    let g = num(i - 1).unwrap_or(0.0);
                    color = [g; 3];
                }
                "rg" if i >= 3 => {
                    color = [
                        num(i - 3).unwrap_or(0.0),
                        num(i - 2).unwrap_or(0.0),
                        num(i - 1).unwrap_or(0.0),
                    ]
                }
                _ => {}
            }
        }
        let mk = w
            .get(b"MK")
            .ok()
            .and_then(|o| deref(doc, o))
            .and_then(|o| o.as_dict().ok());
        let rgb = |k: &[u8]| -> Option<[f32; 3]> {
            let a = mk?.get(k).ok()?.as_array().ok()?;
            let v: Vec<f32> = a.iter().filter_map(|o| o.as_float().ok()).collect();
            match v.len() {
                1 => Some([v[0]; 3]),
                3 => Some([v[0], v[1], v[2]]),
                _ => None,
            }
        };
        Look {
            size: size.max(0.0),
            color,
            background: rgb(b"BG"),
            border: rgb(b"BC"),
        }
    }
}

/// Content of a text or choice widget's appearance (`/Tx BMC … EMC`).
fn appearance(kind: &FieldKind, value: &str, rect: [f32; 4], look: &Look, quad: i64) -> String {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let mut s = String::new();
    if let Some([r, g, b]) = look.background {
        s.push_str(&format!("{r} {g} {b} rg 0 0 {w} {h} re f\n"));
    }
    if let Some([r, g, b]) = look.border {
        s.push_str(&format!(
            "{r} {g} {b} RG 1 w 0.5 0.5 {} {} re S\n",
            w - 1.0,
            h - 1.0
        ));
    }
    s.push_str("/Tx BMC\nq 1 1 ");
    s.push_str(&format!(
        "{} {} re W n\n",
        (w - 2.0).max(0.0),
        (h - 2.0).max(0.0)
    ));
    let [r, g, b] = look.color;
    let color = format!("{r} {g} {b} rg");
    match kind {
        FieldKind::Text {
            multiline: true, ..
        } => {
            let size = if look.size > 0.0 { look.size } else { 10.0 };
            let lines = wrap(value, size, w - 4.0);
            let lead = size * 1.15;
            let mut y = h - 2.0 - size;
            s.push_str(&format!("BT /{FONT} {size} Tf {color}\n"));
            for line in lines {
                if y < -size {
                    break;
                }
                let x = aligned(quad, w, text_width(&line, size));
                s.push_str(&format!("1 0 0 1 {x} {y} Tm ({}) Tj\n", pdf_text(&line)));
                y -= lead;
            }
            s.push_str("ET\n");
        }
        FieldKind::Text {
            comb: true,
            max_len: Some(m),
            ..
        } => {
            let size = fit_size(look.size, h, None);
            let cell = w / *m as f32;
            let y = baseline(h, size);
            s.push_str(&format!("BT /{FONT} {size} Tf {color}\n"));
            for (i, c) in value.chars().take(*m).enumerate() {
                let t = c.to_string();
                let x = cell * i as f32 + (cell - text_width(&t, size)) / 2.0;
                s.push_str(&format!("1 0 0 1 {x} {y} Tm ({}) Tj\n", pdf_text(&t)));
            }
            s.push_str("ET\n");
        }
        FieldKind::Choice {
            combo: false,
            options,
            ..
        } => {
            // A list: the options top-down, the chosen one marked.
            let size = if look.size > 0.0 { look.size } else { 10.0 };
            let lead = size * 1.15;
            let mut y = h - 1.0;
            for o in options {
                if y < 0.0 {
                    break;
                }
                if o.value == value {
                    s.push_str(&format!(
                        "0.6 0.75 0.95 rg 1 {} {} {lead} re f\n",
                        y - lead,
                        w - 2.0
                    ));
                }
                s.push_str(&format!(
                    "BT /{FONT} {size} Tf {color} 1 0 0 1 2 {} Tm ({}) Tj ET\n",
                    y - size,
                    pdf_text(&o.label)
                ));
                y -= lead;
            }
        }
        _ => {
            // One line: a text field, or a drop-down showing the chosen option's label.
            let shown = match kind {
                FieldKind::Choice { options, .. } => options
                    .iter()
                    .find(|o| o.value == value)
                    .map_or(value.to_string(), |o| o.label.clone()),
                FieldKind::Text { password: true, .. } => "*".repeat(value.chars().count()),
                _ => value.to_string(),
            };
            let size = fit_size(look.size, h, Some((&shown, w - 4.0)));
            let x = aligned(quad, w, text_width(&shown, size));
            s.push_str(&format!(
                "BT /{FONT} {size} Tf {color} 1 0 0 1 {x} {} Tm ({}) Tj ET\n",
                baseline(h, size),
                pdf_text(&shown)
            ));
        }
    }
    s.push_str("Q\nEMC\n");
    s
}

/// The `/DA` size, or (0, auto) one that fits the box's height and, given, the text's width.
fn fit_size(da: f32, h: f32, fit: Option<(&str, f32)>) -> f32 {
    if da > 0.0 {
        return da;
    }
    let mut size = ((h - 4.0) / 1.15).clamp(4.0, 12.0);
    if let Some((text, width)) = fit {
        let tw = text_width(text, size);
        if tw > width && tw > 0.0 {
            size = (size * width / tw).max(4.0);
        }
    }
    size
}

fn baseline(h: f32, size: f32) -> f32 {
    ((h - size) / 2.0 + size * 0.22).max(1.0)
}

fn aligned(quad: i64, w: f32, tw: f32) -> f32 {
    match quad {
        1 => ((w - tw) / 2.0).max(2.0),
        2 => (w - 2.0 - tw).max(2.0),
        _ => 2.0,
    }
}

/// Lines of `text` that fit `width` at `size` (breaking at spaces, and at newlines).
fn wrap(text: &str, size: f32, width: f32) -> Vec<String> {
    let mut out = Vec::new();
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty() && text_width(&candidate, size) > width {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        out.push(line.trim_end_matches('\r').to_string());
    }
    out
}

/// Helvetica advance widths (1/1000 em) for ASCII 32..=126.
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

fn text_width(text: &str, size: f32) -> f32 {
    let units: u32 = text
        .chars()
        .map(|c| match c as u32 {
            n @ 32..=126 => u32::from(HELV[(n - 32) as usize]),
            _ => 556,
        })
        .sum();
    units as f32 * size / 1000.0
}

/// `text` for a `( … ) Tj` in WinAnsi: escaped, anything WinAnsi lacks as `?`.
fn pdf_text(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let b: u8 = match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                c as u8
            }
            ' '..='~' => c as u8,
            '\u{A0}'..='\u{FF}' => c as u32 as u8,
            '€' => 0x80,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            _ => b'?',
        };
        if b < 0x80 {
            out.push(b as char);
        } else {
            out.push_str(&format!("\\{b:03o}"));
        }
    }
    out
}

#[cfg(any(test, feature = "test-support"))]
/// A one-page form: a text field, a multi-line text field, a check box, two radio buttons
/// (a parent with two kids), a drop-down and a read-only field. Invented values only.
pub fn write_test_form(path: &Path) {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let page_id = doc.new_object_id();
    let check_ap = |doc: &mut Document, on: &str| {
        let on_s = doc.add_object(Stream::new(
            dictionary! {"BBox" => vec![0.into(), 0.into(), 12.into(), 12.into()]},
            b"0 0 12 12 re f".to_vec(),
        ));
        let off_s = doc.add_object(Stream::new(
            dictionary! {"BBox" => vec![0.into(), 0.into(), 12.into(), 12.into()]},
            Vec::new(),
        ));
        let mut n = Dictionary::new();
        n.set(on, Object::Reference(on_s));
        n.set("Off", Object::Reference(off_s));
        dictionary! { "N" => n }
    };
    let name = Object::string_literal;
    let mut annots = Vec::new();
    let mut fields = Vec::new();
    let text = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => name("client_name"),
        "TU" => name("Client name"), "DA" => name("/Helv 0 Tf 0 g"),
        "Rect" => vec![72.into(), 700.into(), 300.into(), 720.into()], "P" => page_id,
    });
    let notes = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => name("notes"),
        "Ff" => 4096, "DA" => name("/Helv 10 Tf 0 g"),
        "Rect" => vec![72.into(), 560.into(), 300.into(), 640.into()], "P" => page_id,
    });
    let ap = check_ap(&mut doc, "Yes");
    let check = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn", "T" => name("signed"),
        "V" => "Off", "AS" => "Off", "AP" => ap,
        "Rect" => vec![72.into(), 520.into(), 84.into(), 532.into()], "P" => page_id,
    });
    let radio = doc.new_object_id();
    let ap1 = check_ap(&mut doc, "Married");
    let r1 = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "Parent" => radio, "AS" => "Off", "AP" => ap1,
        "Rect" => vec![72.into(), 480.into(), 84.into(), 492.into()], "P" => page_id,
    });
    let ap2 = check_ap(&mut doc, "Single");
    let r2 = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "Parent" => radio, "AS" => "Single", "AP" => ap2,
        "Rect" => vec![120.into(), 480.into(), 132.into(), 492.into()], "P" => page_id,
    });
    doc.objects.insert(
        radio,
        Object::Dictionary(dictionary! {
            "FT" => "Btn", "T" => name("status"), "Ff" => 1 << 15, "V" => "Single",
            "Kids" => vec![r1.into(), r2.into()],
        }),
    );
    let choice = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Ch", "T" => name("year"), "Ff" => 1 << 17,
        "Opt" => vec![Object::Array(vec![name("2025"), name("Tax year 2025")]), name("2024")],
        "V" => name("2024"), "DA" => name("/Helv 9 Tf 0 g"),
        "Rect" => vec![72.into(), 440.into(), 200.into(), 456.into()], "P" => page_id,
    });
    let locked = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => name("form_id"),
        "Ff" => 1, "V" => name("F-100"),
        "Rect" => vec![400.into(), 700.into(), 500.into(), 720.into()], "P" => page_id,
    });
    annots.extend([text, notes, check, r1, r2, choice, locked].map(Object::Reference));
    fields.extend([text, notes, check, radio, choice, locked].map(Object::Reference));
    let content = doc.add_object(Stream::new(Dictionary::new(), Vec::new()));
    doc.objects.insert(
        page_id,
        Object::Dictionary(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Annots" => annots,
        }),
    );
    doc.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! {"Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1},
        ),
    );
    let form = doc.add_object(dictionary! { "Fields" => fields, "DA" => name("/Helv 0 Tf 0 g") });
    let cat =
        doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id, "AcroForm" => form});
    doc.trailer.set("Root", cat);
    doc.save(path).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_read_with_kind_value_and_place() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("f.pdf");
        write_test_form(&p);
        let f = read(&p).unwrap();
        let names: Vec<&str> = f.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "client_name",
                "form_id",
                "notes",
                "signed",
                "status",
                "year"
            ]
        );
        let by = |n: &str| f.iter().find(|x| x.name == n).unwrap();
        assert_eq!(by("client_name").title(), "Client name");
        assert!(matches!(
            by("notes").kind,
            FieldKind::Text {
                multiline: true,
                ..
            }
        ));
        assert_eq!(by("signed").kind, FieldKind::Check { on: "Yes".into() });
        assert_eq!(by("signed").value, "Off");
        assert_eq!(
            by("status").kind,
            FieldKind::Radio {
                options: vec!["Married".into(), "Single".into()]
            }
        );
        assert_eq!(by("status").value, "Single");
        assert_eq!(by("status").widgets.len(), 2);
        let FieldKind::Choice { options, combo, .. } = &by("year").kind else {
            panic!()
        };
        assert!(*combo);
        assert_eq!(options[0].label, "Tax year 2025");
        assert_eq!(by("year").value, "2024");
        assert!(by("form_id").read_only);
        assert_eq!(
            by("client_name").widgets[0].rect,
            [72.0, 700.0, 300.0, 720.0]
        );
    }

    #[test]
    fn filled_values_are_kept_and_drawn() {
        let t = tempfile::tempdir().unwrap();
        let (src, out) = (t.path().join("f.pdf"), t.path().join("o.pdf"));
        write_test_form(&src);
        let vals = [
            ("client_name", "Example Farms (Test) LLC"),
            (
                "notes",
                "First line\nSecond line that is long enough to wrap onto another line",
            ),
            ("signed", "Yes"),
            ("status", "Married"),
            ("year", "2025"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()));
        assert_eq!(fill(&src, &out, &vals).unwrap(), 5);
        let f = read(&out).unwrap();
        for (n, v) in &vals {
            assert_eq!(&f.iter().find(|x| &x.name == n).unwrap().value, v, "{n}");
        }
        let doc = Document::load(&out).unwrap();
        // The text widget's appearance shows the value, parentheses escaped.
        let found = found(&doc);
        let w = found
            .iter()
            .find(|x| x.field.name == "client_name")
            .unwrap()
            .widgets[0];
        let ap = doc
            .get_dictionary(w)
            .unwrap()
            .get(b"AP")
            .unwrap()
            .as_dict()
            .unwrap();
        let n = ap.get(b"N").unwrap().as_reference().unwrap();
        let Object::Stream(s) = doc.get_object(n).unwrap() else {
            panic!()
        };
        let text = String::from_utf8_lossy(&s.content);
        assert!(text.contains("(Example Farms \\(Test\\) LLC) Tj"), "{text}");
        // The drop-down shows the label, not the stored value.
        let w = found
            .iter()
            .find(|x| x.field.name == "year")
            .unwrap()
            .widgets[0];
        let n = doc
            .get_dictionary(w)
            .unwrap()
            .get(b"AP")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"N")
            .unwrap()
            .as_reference()
            .unwrap();
        let Object::Stream(s) = doc.get_object(n).unwrap() else {
            panic!()
        };
        assert!(String::from_utf8_lossy(&s.content).contains("(Tax year 2025) Tj"));
        // Radio: the chosen widget is on, the other off.
        let r = found.iter().find(|x| x.field.name == "status").unwrap();
        let states: Vec<Vec<u8>> = r
            .widgets
            .iter()
            .map(|w| {
                doc.get_dictionary(*w)
                    .unwrap()
                    .get(b"AS")
                    .unwrap()
                    .as_name()
                    .unwrap()
                    .to_vec()
            })
            .collect();
        assert_eq!(states, [b"Married".to_vec(), b"Off".to_vec()]);
        // Filling again with the same values changes nothing.
        let again = t.path().join("a.pdf");
        assert_eq!(fill(&out, &again, &vals).unwrap(), 0);
    }

    #[test]
    fn unicode_values_round_trip_and_bad_fills_write_nothing() {
        let t = tempfile::tempdir().unwrap();
        let (src, out) = (t.path().join("f.pdf"), t.path().join("o.pdf"));
        write_test_form(&src);
        fill(
            &src,
            &out,
            &[("client_name".into(), "Café Ñandú €5".into())],
        )
        .unwrap();
        assert_eq!(
            read(&out)
                .unwrap()
                .iter()
                .find(|f| f.name == "client_name")
                .unwrap()
                .value,
            "Café Ñandú €5"
        );
        let bad = t.path().join("bad.pdf");
        for vals in [
            vec![("form_id".to_string(), "X".to_string())],
            vec![("nope".to_string(), "X".to_string())],
            vec![("status".to_string(), "Widowed".to_string())],
        ] {
            assert!(fill(&src, &bad, &vals).is_err(), "{vals:?}");
            assert!(!bad.exists());
        }
    }

    #[test]
    fn flatten_burns_the_filled_value_in() {
        let t = tempfile::tempdir().unwrap();
        let (src, out, flat) = (
            t.path().join("f.pdf"),
            t.path().join("o.pdf"),
            t.path().join("flat.pdf"),
        );
        write_test_form(&src);
        fill(
            &src,
            &out,
            &[("client_name".into(), "Sample Client".into())],
        )
        .unwrap();
        crate::flatten::flatten(&out, &flat).unwrap();
        let d = Document::load(&flat).unwrap();
        assert!(d.catalog().unwrap().get(b"AcroForm").is_err());
        assert!(read_doc(&d).is_empty());
        let bytes = d
            .get_page_content(*d.get_pages().values().next().unwrap())
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("Do"));
        // The appearance holding the text is still in the file, as a form XObject.
        let any = d.objects.values().any(|o| matches!(o, Object::Stream(s) if String::from_utf8_lossy(&s.content).contains("(Sample Client) Tj")));
        assert!(any);
    }

    #[test]
    fn xfa_list_index_and_multi_value_check_boxes() {
        let t = tempfile::tempdir().unwrap();
        let (src, out) = (t.path().join("f.pdf"), t.path().join("o.pdf"));
        write_test_form(&src);
        let mut doc = Document::load(&src).unwrap();
        // XFA beside the AcroForm, a list with /I, and a check box with two boxes ("1", "2").
        let form = doc
            .catalog()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_dictionary_mut(form)
            .unwrap()
            .set("XFA", Object::string_literal("<xdp/>"));
        let year = found(&doc)
            .into_iter()
            .find(|f| f.field.name == "year")
            .unwrap()
            .id;
        doc.get_dictionary_mut(year)
            .unwrap()
            .set("I", vec![Object::Integer(1)]);
        let page = *doc.get_pages().values().next().unwrap();
        let mk = |doc: &mut Document, on: &str, x: i64, parent: ObjectId| {
            let s = doc.add_object(Stream::new(Dictionary::new(), Vec::new()));
            let mut n = Dictionary::new();
            n.set(on, s);
            n.set("Off", s);
            doc.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Widget", "Parent" => parent, "AS" => "Off",
                "AP" => dictionary! { "N" => n },
                "Rect" => vec![x.into(), 400.into(), (x + 12).into(), 412.into()],
            })
        };
        let filing = doc.new_object_id();
        let (a, b) = (
            mk(&mut doc, "1", 72, filing),
            mk(&mut doc, "2", 120, filing),
        );
        doc.objects.insert(
            filing,
            Object::Dictionary(dictionary! {
                "FT" => "Btn", "T" => Object::string_literal("filing"), "Kids" => vec![a.into(), b.into()],
            }),
        );
        doc.get_dictionary_mut(page)
            .unwrap()
            .get_mut(b"Annots")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .extend([a.into(), b.into()]);
        doc.get_dictionary_mut(form)
            .unwrap()
            .get_mut(b"Fields")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(filing.into());
        doc.save(&src).unwrap();

        let f = read(&src).unwrap();
        let filing_f = f.iter().find(|f| f.name == "filing").unwrap();
        assert_eq!(
            filing_f.kind,
            FieldKind::Radio {
                options: vec!["1".into(), "2".into()]
            }
        );
        fill(
            &src,
            &out,
            &[
                ("filing".into(), "2".into()),
                ("year".into(), "2025".into()),
            ],
        )
        .unwrap();
        let d = Document::load(&out).unwrap();
        let form = d
            .catalog()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_reference()
            .unwrap();
        assert!(!d.get_dictionary(form).unwrap().has(b"XFA"), "XFA dropped");
        let fs = found(&d);
        let y = fs.iter().find(|f| f.field.name == "year").unwrap();
        let i = d
            .get_dictionary(y.id)
            .unwrap()
            .get(b"I")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(i, &vec![Object::Integer(0)], "/I follows the value");
        let r = fs.iter().find(|f| f.field.name == "filing").unwrap();
        let states: Vec<Vec<u8>> = r
            .widgets
            .iter()
            .map(|w| {
                d.get_dictionary(*w)
                    .unwrap()
                    .get(b"AS")
                    .unwrap()
                    .as_name()
                    .unwrap()
                    .to_vec()
            })
            .collect();
        assert_eq!(
            states,
            [b"Off".to_vec(), b"2".to_vec()],
            "the second box is on"
        );
    }

    #[test]
    fn turned_widgets_nameless_groups_and_missing_appearances() {
        let t = tempfile::tempdir().unwrap();
        let (src, out) = (t.path().join("f.pdf"), t.path().join("o.pdf"));
        write_test_form(&src);
        let mut doc = Document::load(&src).unwrap();
        let fs = found(&doc);
        let id = |n: &str| fs.iter().find(|f| f.field.name == n).unwrap().id;
        // client_name turned 90°; notes filled by "another program": a value, no appearance,
        // NeedAppearances false.
        doc.get_dictionary_mut(id("client_name"))
            .unwrap()
            .set("MK", dictionary! { "R" => 90 });
        doc.get_dictionary_mut(id("notes"))
            .unwrap()
            .set("V", Object::string_literal("Line one\rLine two"));
        // A nameless group holding a named field.
        let page = *doc.get_pages().values().next().unwrap();
        let group = doc.new_object_id();
        let inner = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("inner"),
            "Parent" => group, "Rect" => vec![300.into(), 300.into(), 400.into(), 320.into()],
        });
        doc.objects.insert(
            group,
            Object::Dictionary(dictionary! { "Kids" => vec![inner.into()] }),
        );
        let outer = doc.add_object(dictionary! {
            "T" => Object::string_literal("outer"), "Kids" => vec![group.into()],
        });
        doc.get_dictionary_mut(page)
            .unwrap()
            .get_mut(b"Annots")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(inner.into());
        let form = doc
            .catalog()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_dictionary_mut(form)
            .unwrap()
            .get_mut(b"Fields")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(outer.into());
        doc.save(&src).unwrap();

        assert!(read(&src).unwrap().iter().any(|f| f.name == "outer.inner"));
        fill(&src, &out, &[("client_name".into(), "Turned".into())]).unwrap();
        let mut d = Document::load(&out).unwrap();
        let fs = found(&d);
        let w = fs
            .iter()
            .find(|f| f.field.name == "client_name")
            .unwrap()
            .widgets[0];
        let n = d
            .get_dictionary(w)
            .unwrap()
            .get(b"AP")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"N")
            .unwrap()
            .as_reference()
            .unwrap();
        let Object::Stream(st) = d.get_object(n).unwrap() else {
            panic!()
        };
        let bbox: Vec<f32> = st
            .dict
            .get(b"BBox")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o.as_float().unwrap())
            .collect();
        assert_eq!(bbox, [0.0, 0.0, 20.0, 228.0], "drawn in the turned frame");
        // notes, the drop-down and the read-only form_id have values and no look.
        assert_eq!(draw_missing_appearances(&mut d), 3);
        let flat = t.path().join("flat.pdf");
        d.save(&out).unwrap();
        crate::flatten::flatten(&out, &flat).unwrap();
        let fd = Document::load(&flat).unwrap();
        let has = |txt: &str| {
            fd.objects.values().any(|o| matches!(o, Object::Stream(s) if String::from_utf8_lossy(&s.content).contains(txt)))
        };
        assert!(
            has("(Line one) Tj") && has("(Line two) Tj"),
            "CR breaks the line"
        );
        assert!(has("(Turned) Tj"));
    }

    #[test]
    fn latin1_state_names_round_trip() {
        assert_eq!(name_text(b"S\xEDn"), "Sín");
        assert_eq!(name_text("Sí".as_bytes()), "Sí");
    }

    #[test]
    fn wrapping_and_widths() {
        assert_eq!(text_width("i", 1000.0), 222.0);
        let lines = wrap("aaa bbb ccc", 10.0, text_width("aaa bbb", 10.0) + 0.1);
        assert_eq!(lines, ["aaa bbb", "ccc"]);
        assert_eq!(pdf_text("é(€)"), "\\351\\(\\200\\)");
    }
}
