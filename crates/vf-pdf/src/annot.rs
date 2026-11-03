//! Annotations, tickmarks and stamps written as real PDF annotations (P04 steps 2–3), so any
//! PDF app shows them: every one carries an appearance stream (`/AP /N`), its author (`/T`),
//! dates (`/M`, `/CreationDate`), the Print flag and a unique name (`/NM`).
//!
//! Coordinates are PDF user space (points, origin bottom left of the MediaBox); the Viewer
//! converts from what it shows. Appearances of stamps, tickmarks, notes and text boxes are
//! turned against the page's `/Rotate`, so they read upright on a rotated page.

use std::path::Path;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};

/// RGB, 0–1.
pub type Rgb = [f32; 3];

pub const YELLOW: Rgb = [1.0, 0.91, 0.6];
pub const RED: Rgb = [0.75, 0.1, 0.1];
pub const ACCENT: Rgb = [0.12, 0.34, 0.66];
const TICK_FILL: Rgb = [0.867, 0.949, 0.894];
const TICK_INK: Rgb = [0.114, 0.361, 0.22];

/// Tickmark glyphs (UI_SPEC §4 palette). Drawn as vector paths and letters, never font
/// symbols, so they look the same in every viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// ✓ agreed
    Check,
    /// ✗ exception
    Cross,
    /// ⊘ not applicable
    Void,
    /// ƒ footed
    Footed,
    /// T/B tied to books
    TiedToBooks,
    /// PY agreed to prior year
    PriorYear,
    /// CY agreed to current year
    CurrentYear,
}

impl Tick {
    pub const ALL: [Tick; 7] = [
        Tick::Check,
        Tick::Cross,
        Tick::Void,
        Tick::Footed,
        Tick::TiedToBooks,
        Tick::PriorYear,
        Tick::CurrentYear,
    ];

    /// The words a tooltip and `/Contents` use.
    pub fn meaning(self) -> &'static str {
        match self {
            Tick::Check => "agreed",
            Tick::Cross => "exception",
            Tick::Void => "not applicable",
            Tick::Footed => "footed",
            Tick::TiedToBooks => "tied to books",
            Tick::PriorYear => "agreed to prior year",
            Tick::CurrentYear => "agreed to current year",
        }
    }

    /// Short label (letters for the letter ticks).
    pub fn label(self) -> &'static str {
        match self {
            Tick::Check => "check",
            Tick::Cross => "cross",
            Tick::Void => "void",
            Tick::Footed => "f",
            Tick::TiedToBooks => "T/B",
            Tick::PriorYear => "PY",
            Tick::CurrentYear => "CY",
        }
    }
}

/// Stamp presets (UI_SPEC §4). `{date}` and `{initials}` are filled in by [`Preset::text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    ClientCopy,
    Draft,
    Filed,
    Received,
    Reviewed,
}

impl Preset {
    pub const ALL: [Preset; 5] = [
        Preset::ClientCopy,
        Preset::Draft,
        Preset::Filed,
        Preset::Received,
        Preset::Reviewed,
    ];

    /// The list label, with its placeholders.
    pub fn label(self) -> &'static str {
        match self {
            Preset::ClientCopy => "Client Copy",
            Preset::Draft => "Draft",
            Preset::Filed => "Filed {date}",
            Preset::Received => "Received {date}",
            Preset::Reviewed => "Reviewed {initials} {date}",
        }
    }

    /// The stamped words: upper case, `{date}` as MM/DD/YYYY.
    pub fn text(self, date_ymd: &str, initials: &str) -> String {
        let d = match date_ymd.get(..10) {
            Some(d) if d.len() == 10 => format!("{}/{}/{}", &d[5..7], &d[8..10], &d[..4]),
            _ => date_ymd.to_string(),
        };
        self.label()
            .replace("{date}", &d)
            .replace("{initials}", initials)
            .to_uppercase()
    }
}

/// Where a stamp goes on the page as shown (UI placement presets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    TopLeft,
    TopCenter,
    TopRight,
    Center,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Placement {
    pub const ALL: [Placement; 7] = [
        Placement::TopLeft,
        Placement::TopCenter,
        Placement::TopRight,
        Placement::Center,
        Placement::BottomLeft,
        Placement::BottomCenter,
        Placement::BottomRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Placement::TopLeft => "Top left",
            Placement::TopCenter => "Top center",
            Placement::TopRight => "Top right",
            Placement::Center => "Center",
            Placement::BottomLeft => "Bottom left",
            Placement::BottomCenter => "Bottom center",
            Placement::BottomRight => "Bottom right",
        }
    }
}

/// A stamp to put on several pages.
#[derive(Debug, Clone, PartialEq)]
pub struct StampSpec {
    pub text: String,
    pub size: f32,
    pub placement: Placement,
    /// 0-based; empty = every page.
    pub pages: Vec<u32>,
    pub color: Rgb,
    pub author: String,
}

/// Puts `spec` on its pages of `src`, writing `out`. `now` is RFC 3339.
pub fn stamp_pages(src: &Path, out: &Path, spec: &StampSpec, now: &str) -> Result<usize, String> {
    let StampSpec {
        text,
        size,
        placement,
        pages,
        color,
        author,
    } = spec;
    let (size, placement, color) = (*size, *placement, *color);
    let doc = Document::load(src).map_err(|e| format!("cannot read the PDF: {e}"))?;
    let ids: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    let chosen: Vec<u32> = if pages.is_empty() {
        (0..ids.len() as u32).collect()
    } else {
        pages.to_vec()
    };
    let (w, h) = stamp_size(text, size);
    let mut annots = Vec::new();
    for p in chosen {
        let Some(&id) = ids.get(p as usize) else {
            return Err(format!("page {} is not in the PDF", p + 1));
        };
        let g = crate::layer::Geometry::of(&doc, id)?;
        let (dw, dh) = g.display();
        let m = 24.0;
        let u = match placement {
            Placement::TopLeft | Placement::BottomLeft => m + w / 2.0,
            Placement::TopRight | Placement::BottomRight => dw - m - w / 2.0,
            _ => dw / 2.0,
        };
        let v = match placement {
            Placement::TopLeft | Placement::TopCenter | Placement::TopRight => m + h / 2.0,
            Placement::Center => dh / 2.0,
            _ => dh - m - h / 2.0,
        };
        annots.push(Annotation {
            page: p,
            kind: Kind::Stamp {
                text: text.clone(),
                at: g.to_user(u, v),
                size,
            },
            color,
            author: author.clone(),
        });
    }
    drop(doc);
    add(src, out, &annots, now)
}

/// What to draw.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// Text markup over word boxes: each `[x0, y0, x1, y1]` in user space.
    Highlight(Vec<[f32; 4]>),
    Underline(Vec<[f32; 4]>),
    StrikeOut(Vec<[f32; 4]>),
    /// Sticky note at a point.
    Note {
        at: [f32; 2],
        text: String,
    },
    /// Text box.
    TextBox {
        rect: [f32; 4],
        text: String,
        size: f32,
    },
    /// Freehand strokes.
    Ink(Vec<Vec<[f32; 2]>>),
    Arrow {
        from: [f32; 2],
        to: [f32; 2],
    },
    /// A tickmark centred at a point, `size` points across.
    Tickmark {
        tick: Tick,
        at: [f32; 2],
        size: f32,
    },
    /// A text stamp (`CLIENT COPY`, `RECEIVED 10/29/2025`) centred at a point.
    Stamp {
        text: String,
        at: [f32; 2],
        size: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    /// 0-based.
    pub page: u32,
    pub kind: Kind,
    pub color: Rgb,
    /// `/T`: initials or user name.
    pub author: String,
}

/// An annotation as read back (Viewer list, tests).
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub page: u32,
    pub subtype: String,
    pub rect: [f32; 4],
    pub author: Option<String>,
    pub contents: Option<String>,
    pub has_appearance: bool,
    pub printable: bool,
}

/// `D:YYYYMMDDHHmmSSZ` from an RFC 3339 UTC time.
pub fn pdf_date(rfc3339: &str) -> String {
    let d: String = rfc3339
        .chars()
        .filter(char::is_ascii_digit)
        .take(14)
        .collect();
    format!("D:{d:0<14}Z")
}

/// Writes `src` plus `annots` to `out`. `now` is RFC 3339.
pub fn add(src: &Path, out: &Path, annots: &[Annotation], now: &str) -> Result<usize, String> {
    let mut doc = Document::load(src).map_err(|e| format!("cannot read the PDF: {e}"))?;
    if doc.is_encrypted() {
        return Err("the PDF is password protected".into());
    }
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    let font = doc.add_object(Dictionary::from_iter(vec![
        ("Type", Object::Name(b"Font".to_vec())),
        ("Subtype", Object::Name(b"Type1".to_vec())),
        ("BaseFont", Object::Name(b"Helvetica-Bold".to_vec())),
        ("Encoding", Object::Name(b"WinAnsiEncoding".to_vec())),
    ]));
    let multiply = doc.add_object(Dictionary::from_iter(vec![
        ("Type", Object::Name(b"ExtGState".to_vec())),
        ("BM", Object::Name(b"Multiply".to_vec())),
    ]));
    let date = pdf_date(now);
    for (n, a) in annots.iter().enumerate() {
        let Some(&page) = pages.get(a.page as usize) else {
            return Err(format!("page {} is not in the PDF", a.page + 1));
        };
        let rotate = crate::layer::page_rotate(&doc, page);
        let built = build(a, rotate, font, multiply);
        let ap = doc.add_object(Stream::new(built.ap_dict, built.ap));
        let mut d = built.dict;
        d.set("Type", Object::Name(b"Annot".to_vec()));
        d.set("Subtype", Object::Name(built.subtype.as_bytes().to_vec()));
        d.set("Rect", rect_obj(built.rect));
        d.set("C", color_obj(a.color));
        d.set("T", text_obj(&a.author));
        d.set("M", text_obj(&date));
        d.set("CreationDate", text_obj(&date));
        d.set("F", 4);
        d.set("P", Object::Reference(page));
        d.set(
            "NM",
            text_obj(&format!("vf-{}-{n}", date.trim_start_matches("D:"))),
        );
        if let Some(c) = &built.contents {
            d.set("Contents", text_obj(c));
        }
        d.set(
            "AP",
            Dictionary::from_iter(vec![("N", Object::Reference(ap))]),
        );
        let id = doc.add_object(d);
        push_annot(&mut doc, page, id)?;
    }
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)
        .map_err(|e| format!("cannot write the PDF: {e}"))?;
    std::fs::write(out, bytes).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(annots.len())
}

/// Every annotation in `path`, page by page.
pub fn read(path: &Path) -> Result<Vec<Found>, String> {
    let doc = Document::load(path).map_err(|e| format!("cannot read the PDF: {e}"))?;
    let mut out = Vec::new();
    for (i, (_, page)) in doc.get_pages().into_iter().enumerate() {
        let Ok(dict) = doc.get_dictionary(page) else {
            continue;
        };
        let annots = match dict.get(b"Annots") {
            Ok(o) => match doc.dereference(o) {
                Ok((_, Object::Array(a))) => a.clone(),
                _ => continue,
            },
            Err(_) => continue,
        };
        for a in annots {
            let Ok((_, Object::Dictionary(d))) = doc.dereference(&a) else {
                continue;
            };
            let s = |k: &[u8]| {
                d.get(k)
                    .ok()
                    .and_then(|o| o.as_str().ok())
                    .map(|b| String::from_utf8_lossy(b).into_owned())
            };
            let rect: Vec<f32> = d
                .get(b"Rect")
                .ok()
                .and_then(|o| o.as_array().ok())
                .map(|a| a.iter().filter_map(|v| v.as_float().ok()).collect())
                .unwrap_or_default();
            out.push(Found {
                page: i as u32,
                subtype: d
                    .get(b"Subtype")
                    .ok()
                    .and_then(|o| o.as_name().ok())
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_default(),
                rect: [
                    rect.first().copied().unwrap_or(0.0),
                    rect.get(1).copied().unwrap_or(0.0),
                    rect.get(2).copied().unwrap_or(0.0),
                    rect.get(3).copied().unwrap_or(0.0),
                ],
                author: s(b"T"),
                contents: s(b"Contents"),
                has_appearance: d
                    .get(b"AP")
                    .ok()
                    .and_then(|o| doc.dereference(o).ok())
                    .and_then(|(_, o)| o.as_dict().ok().cloned())
                    .is_some_and(|ap| ap.has(b"N")),
                printable: d
                    .get(b"F")
                    .ok()
                    .and_then(|o| o.as_i64().ok())
                    .is_some_and(|f| f & 4 != 0),
            });
        }
    }
    Ok(out)
}

struct Built {
    subtype: &'static str,
    rect: [f32; 4],
    dict: Dictionary,
    ap_dict: Dictionary,
    ap: Vec<u8>,
    contents: Option<String>,
}

fn build(a: &Annotation, rotate: i64, font: ObjectId, multiply: ObjectId) -> Built {
    let mut dict = Dictionary::new();
    let (subtype, rect, ops, upright, contents): (_, [f32; 4], Vec<Operation>, bool, _) =
        match &a.kind {
            Kind::Highlight(q) | Kind::Underline(q) | Kind::StrikeOut(q) => {
                let rect = union(q);
                dict.set("QuadPoints", quads_obj(q));
                let ops = markup_ops(&a.kind, q, a.color, rect);
                let st = match &a.kind {
                    Kind::Highlight(_) => "Highlight",
                    Kind::Underline(_) => "Underline",
                    _ => "StrikeOut",
                };
                (st, rect, ops, false, None)
            }
            Kind::Note { at, text } => {
                dict.set("Name", Object::Name(b"Comment".to_vec()));
                let rect = [at[0], at[1] - 20.0, at[0] + 20.0, at[1]];
                ("Text", rect, note_icon(a.color), true, Some(text.clone()))
            }
            Kind::TextBox { rect, text, size } => {
                dict.set(
                    "DA",
                    Object::string_literal(format!(
                        "/Helv {size} Tf {} {} {} rg",
                        a.color[0], a.color[1], a.color[2]
                    )),
                );
                let ops = text_box_ops(*rect, text, *size, a.color, rotate);
                ("FreeText", *rect, ops, true, Some(text.clone()))
            }
            Kind::Ink(strokes) => {
                let pts: Vec<[f32; 2]> = strokes.iter().flatten().copied().collect();
                let rect = pad(bounds(&pts), 2.0);
                dict.set(
                    "InkList",
                    Object::Array(
                        strokes
                            .iter()
                            .map(|s| {
                                Object::Array(
                                    s.iter().flat_map(|p| [p[0].into(), p[1].into()]).collect(),
                                )
                            })
                            .collect(),
                    ),
                );
                ("Ink", rect, ink_ops(strokes, a.color, rect), false, None)
            }
            Kind::Arrow { from, to } => {
                let rect = pad(bounds(&[*from, *to]), 8.0);
                dict.set(
                    "L",
                    Object::Array(vec![
                        from[0].into(),
                        from[1].into(),
                        to[0].into(),
                        to[1].into(),
                    ]),
                );
                dict.set(
                    "LE",
                    Object::Array(vec![
                        Object::Name(b"None".to_vec()),
                        Object::Name(b"OpenArrow".to_vec()),
                    ]),
                );
                (
                    "Line",
                    rect,
                    arrow_ops(*from, *to, a.color, rect),
                    false,
                    None,
                )
            }
            Kind::Tickmark { tick, at, size } => {
                dict.set("Name", Object::Name(b"VfTickmark".to_vec()));
                let h = size / 2.0;
                let rect = [at[0] - h, at[1] - h, at[0] + h, at[1] + h];
                (
                    "Stamp",
                    rect,
                    tick_ops(*tick, *size),
                    true,
                    Some(format!("Tickmark: {}, {}", tick.meaning(), a.author)),
                )
            }
            Kind::Stamp { text, at, size } => {
                dict.set("Name", Object::Name(b"VfStamp".to_vec()));
                let (w, h) = stamp_size(text, *size);
                // Display-space size; on a page turned 90°/270° the user-space box is turned.
                let (uw, uh) = if rotate % 180 == 0 { (w, h) } else { (h, w) };
                let rect = [
                    at[0] - uw / 2.0,
                    at[1] - uh / 2.0,
                    at[0] + uw / 2.0,
                    at[1] + uh / 2.0,
                ];
                (
                    "Stamp",
                    rect,
                    stamp_ops(text, *size, a.color, w, h),
                    true,
                    Some(text.clone()),
                )
            }
        };
    // Appearance: drawn in a local box; upright ones are drawn in display orientation and
    // turned back by `/Matrix` against the page rotation.
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let (bw, bh, matrix) = if upright {
        let (dw, dh) = if rotate % 180 == 0 { (w, h) } else { (h, w) };
        let m: [f32; 6] = match rotate {
            90 => [0.0, 1.0, -1.0, 0.0, 0.0, 0.0],
            180 => [-1.0, 0.0, 0.0, -1.0, 0.0, 0.0],
            270 => [0.0, -1.0, 1.0, 0.0, 0.0, 0.0],
            _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        };
        (dw, dh, m)
    } else {
        (w, h, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
    };
    let mut res = Dictionary::new();
    res.set(
        "Font",
        Dictionary::from_iter(vec![
            ("Helv", Object::Reference(font)),
            ("VfB", Object::Reference(font)),
        ]),
    );
    res.set(
        "ExtGState",
        Dictionary::from_iter(vec![("Mul", Object::Reference(multiply))]),
    );
    let ap_dict = Dictionary::from_iter(vec![
        ("Type", Object::Name(b"XObject".to_vec())),
        ("Subtype", Object::Name(b"Form".to_vec())),
        (
            "BBox",
            Object::Array(vec![0.into(), 0.into(), bw.into(), bh.into()]),
        ),
        (
            "Matrix",
            Object::Array(matrix.iter().map(|v| (*v).into()).collect()),
        ),
        ("Resources", Object::Dictionary(res)),
    ]);
    let ap = Content { operations: ops }.encode().unwrap_or_default();
    Built {
        subtype,
        rect,
        dict,
        ap_dict,
        ap,
        contents,
    }
}

// ---------------------------------------------------------------------------------------------
// Appearances (local coordinates: 0,0 at the box's lower left)

fn op(name: &str, args: Vec<Object>) -> Operation {
    Operation::new(name, args)
}

fn nums(v: &[f32]) -> Vec<Object> {
    v.iter().map(|x| (*x).into()).collect()
}

fn fill(c: Rgb) -> Operation {
    op("rg", nums(&c))
}

fn stroke(c: Rgb) -> Operation {
    op("RG", nums(&c))
}

fn markup_ops(kind: &Kind, quads: &[[f32; 4]], c: Rgb, rect: [f32; 4]) -> Vec<Operation> {
    let (ox, oy) = (rect[0], rect[1]);
    let mut v = vec![op("q", vec![])];
    match kind {
        Kind::Highlight(_) => {
            v.push(op("gs", vec![Object::Name(b"Mul".to_vec())]));
            v.push(fill(c));
            for q in quads {
                v.push(op(
                    "re",
                    nums(&[q[0] - ox, q[1] - oy, q[2] - q[0], q[3] - q[1]]),
                ));
            }
            v.push(op("f", vec![]));
        }
        _ => {
            v.push(stroke(c));
            for q in quads {
                let lw = ((q[3] - q[1]) / 14.0).max(0.75);
                let y = if matches!(kind, Kind::Underline(_)) {
                    q[1] + lw - oy
                } else {
                    (q[1] + q[3]) / 2.0 - oy
                };
                v.push(op("w", nums(&[lw])));
                v.push(op("m", nums(&[q[0] - ox, y])));
                v.push(op("l", nums(&[q[2] - ox, y])));
                v.push(op("S", vec![]));
            }
        }
    }
    v.push(op("Q", vec![]));
    v
}

fn note_icon(c: Rgb) -> Vec<Operation> {
    vec![
        op("q", vec![]),
        fill(c),
        stroke([0.55, 0.45, 0.1]),
        op("w", nums(&[1.0])),
        op("re", nums(&[1.0, 1.0, 18.0, 18.0])),
        op("B", vec![]),
        op("w", nums(&[1.2])),
        op("m", nums(&[5.0, 13.0])),
        op("l", nums(&[15.0, 13.0])),
        op("m", nums(&[5.0, 9.5])),
        op("l", nums(&[15.0, 9.5])),
        op("m", nums(&[5.0, 6.0])),
        op("l", nums(&[12.0, 6.0])),
        op("S", vec![]),
        op("Q", vec![]),
    ]
}

/// Helvetica-Bold advance, em (an average good enough to size boxes; text is clipped to the
/// box anyway).
const BOLD_EM: f32 = 0.6;
const REGULAR_EM: f32 = 0.52;

fn text_box_ops(rect: [f32; 4], text: &str, size: f32, c: Rgb, rotate: i64) -> Vec<Operation> {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let (w, h) = if rotate % 180 == 0 { (w, h) } else { (h, w) };
    let per_line = ((w - 6.0) / (size * REGULAR_EM)).max(1.0) as usize;
    let mut v = vec![
        op("q", vec![]),
        op("re", nums(&[0.0, 0.0, w, h])),
        op("W", vec![]),
        op("n", vec![]),
        op("BT", vec![]),
        fill(c),
        op("Tf", vec![Object::Name(b"Helv".to_vec()), size.into()]),
    ];
    let mut y = h - size - 2.0;
    for line in wrap(text, per_line) {
        v.push(op("Tm", nums(&[1.0, 0.0, 0.0, 1.0, 3.0, y])));
        v.push(op("Tj", vec![win_ansi_obj(&line)]));
        y -= size * 1.2;
        if y < 0.0 {
            break;
        }
    }
    v.push(op("ET", vec![]));
    v.push(op("Q", vec![]));
    v
}

fn ink_ops(strokes: &[Vec<[f32; 2]>], c: Rgb, rect: [f32; 4]) -> Vec<Operation> {
    let mut v = vec![
        op("q", vec![]),
        stroke(c),
        op("w", nums(&[1.5])),
        op("J", vec![1.into()]),
        op("j", vec![1.into()]),
    ];
    for s in strokes {
        for (i, p) in s.iter().enumerate() {
            let xy = [p[0] - rect[0], p[1] - rect[1]];
            v.push(op(if i == 0 { "m" } else { "l" }, nums(&xy)));
        }
    }
    v.push(op("S", vec![]));
    v.push(op("Q", vec![]));
    v
}

fn arrow_ops(from: [f32; 2], to: [f32; 2], c: Rgb, rect: [f32; 4]) -> Vec<Operation> {
    let (fx, fy, tx, ty) = (
        from[0] - rect[0],
        from[1] - rect[1],
        to[0] - rect[0],
        to[1] - rect[1],
    );
    let ang = (ty - fy).atan2(tx - fx);
    let head = |d: f32| [tx - 8.0 * (ang + d).cos(), ty - 8.0 * (ang + d).sin()];
    let (a, b) = (head(0.45), head(-0.45));
    vec![
        op("q", vec![]),
        stroke(c),
        op("w", nums(&[1.5])),
        op("J", vec![1.into()]),
        op("m", nums(&[fx, fy])),
        op("l", nums(&[tx, ty])),
        op("m", nums(&a)),
        op("l", nums(&[tx, ty])),
        op("l", nums(&b)),
        op("S", vec![]),
        op("Q", vec![]),
    ]
}

/// Circle approximated by four Béziers.
fn circle(cx: f32, cy: f32, r: f32) -> Vec<Operation> {
    let k = 0.5523 * r;
    vec![
        op("m", nums(&[cx + r, cy])),
        op("c", nums(&[cx + r, cy + k, cx + k, cy + r, cx, cy + r])),
        op("c", nums(&[cx - k, cy + r, cx - r, cy + k, cx - r, cy])),
        op("c", nums(&[cx - r, cy - k, cx - k, cy - r, cx, cy - r])),
        op("c", nums(&[cx + k, cy - r, cx + r, cy - k, cx + r, cy])),
    ]
}

fn tick_ops(t: Tick, size: f32) -> Vec<Operation> {
    let (c, r) = (size / 2.0, size / 2.0 - 0.75);
    let mut v = vec![
        op("q", vec![]),
        fill(TICK_FILL),
        stroke(TICK_INK),
        op("w", nums(&[1.0])),
    ];
    v.extend(circle(c, c, r));
    v.push(op("B", vec![]));
    v.push(op("w", nums(&[(size / 12.0).max(1.2)])));
    v.push(op("J", vec![1.into()]));
    v.push(op("j", vec![1.into()]));
    let s = size;
    let line = |v: &mut Vec<Operation>, pts: &[[f32; 2]]| {
        for (i, p) in pts.iter().enumerate() {
            v.push(op(
                if i == 0 { "m" } else { "l" },
                nums(&[p[0] * s, p[1] * s]),
            ));
        }
    };
    match t {
        Tick::Check => {
            line(&mut v, &[[0.27, 0.52], [0.43, 0.34], [0.73, 0.68]]);
            v.push(op("S", vec![]));
        }
        Tick::Cross => {
            line(&mut v, &[[0.32, 0.32], [0.68, 0.68]]);
            line(&mut v, &[[0.32, 0.68], [0.68, 0.32]]);
            v.push(op("S", vec![]));
        }
        Tick::Void => {
            v.extend(circle(c, c, r * 0.55));
            line(&mut v, &[[0.3, 0.3], [0.7, 0.7]]);
            v.push(op("S", vec![]));
        }
        _ => {
            let text = t.label();
            let fs = if text.len() > 1 { s * 0.36 } else { s * 0.55 };
            let tw = text.len() as f32 * fs * BOLD_EM;
            v.push(op("BT", vec![]));
            v.push(fill(TICK_INK));
            v.push(op("Tf", vec![Object::Name(b"VfB".to_vec()), fs.into()]));
            v.push(op("Td", nums(&[c - tw / 2.0, c - fs * 0.35])));
            v.push(op("Tj", vec![win_ansi_obj(text)]));
            v.push(op("ET", vec![]));
        }
    }
    v.push(op("Q", vec![]));
    v
}

/// Stamp box size in display orientation.
fn stamp_size(text: &str, size: f32) -> (f32, f32) {
    (
        text.chars().count() as f32 * size * BOLD_EM + size * 1.2,
        size * 1.9,
    )
}

fn stamp_ops(text: &str, size: f32, c: Rgb, w: f32, h: f32) -> Vec<Operation> {
    let tw = text.chars().count() as f32 * size * BOLD_EM;
    vec![
        op("q", vec![]),
        stroke(c),
        op("w", nums(&[2.0])),
        op("re", nums(&[1.0, 1.0, w - 2.0, h - 2.0])),
        op("S", vec![]),
        op("BT", vec![]),
        fill(c),
        op("Tf", vec![Object::Name(b"VfB".to_vec()), size.into()]),
        op("Td", nums(&[(w - tw) / 2.0, (h - size * 0.7) / 2.0])),
        op("Tj", vec![win_ansi_obj(text)]),
        op("ET", vec![]),
        op("Q", vec![]),
    ]
}

// ---------------------------------------------------------------------------------------------
// Helpers

fn wrap(text: &str, per_line: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines() {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > per_line {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

fn win_ansi_obj(s: &str) -> Object {
    Object::String(crate::layer::win_ansi(s), StringFormat::Literal)
}

/// A text string for an annotation key: PDFDocEncoding when it fits, else UTF-16BE.
fn text_obj(s: &str) -> Object {
    if s.chars().all(|c| (c as u32) < 0x80) {
        Object::string_literal(s)
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(b, StringFormat::Hexadecimal)
    }
}

fn rect_obj(r: [f32; 4]) -> Object {
    Object::Array(r.iter().map(|v| (*v).into()).collect())
}

fn color_obj(c: Rgb) -> Object {
    Object::Array(c.iter().map(|v| (*v).into()).collect())
}

/// QuadPoints in the order viewers expect: upper left, upper right, lower left, lower right.
fn quads_obj(q: &[[f32; 4]]) -> Object {
    Object::Array(
        q.iter()
            .flat_map(|r| {
                [r[0], r[3], r[2], r[3], r[0], r[1], r[2], r[1]]
                    .into_iter()
                    .map(Object::from)
            })
            .collect(),
    )
}

fn union(q: &[[f32; 4]]) -> [f32; 4] {
    q.iter()
        .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |a, r| {
            [
                a[0].min(r[0]),
                a[1].min(r[1]),
                a[2].max(r[2]),
                a[3].max(r[3]),
            ]
        })
}

fn bounds(p: &[[f32; 2]]) -> [f32; 4] {
    p.iter()
        .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |a, p| {
            [
                a[0].min(p[0]),
                a[1].min(p[1]),
                a[2].max(p[0]),
                a[3].max(p[1]),
            ]
        })
}

fn pad(r: [f32; 4], d: f32) -> [f32; 4] {
    [r[0] - d, r[1] - d, r[2] + d, r[3] + d]
}

/// Appends `annot` to the page's `/Annots` (an inline array or a referenced one).
fn push_annot(doc: &mut Document, page: ObjectId, annot: ObjectId) -> Result<(), String> {
    let existing = doc
        .get_dictionary(page)
        .map_err(|e| e.to_string())?
        .get(b"Annots")
        .ok()
        .cloned();
    match existing {
        Some(Object::Reference(id)) => {
            if let Ok(Object::Array(a)) = doc.get_object_mut(id) {
                a.push(Object::Reference(annot));
                return Ok(());
            }
            Err("the page's annotation list is damaged".into())
        }
        Some(Object::Array(mut a)) => {
            a.push(Object::Reference(annot));
            doc.get_dictionary_mut(page)
                .map_err(|e| e.to_string())?
                .set("Annots", a);
            Ok(())
        }
        _ => {
            doc.get_dictionary_mut(page)
                .map_err(|e| e.to_string())?
                .set("Annots", vec![Object::Reference(annot)]);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_texts() {
        assert_eq!(
            Preset::Reviewed.text("2026-11-03T10:00:00Z", "KWK"),
            "REVIEWED KWK 11/03/2026"
        );
        assert_eq!(Preset::ClientCopy.text("2026-11-03", "x"), "CLIENT COPY");
    }

    #[test]
    fn dates_and_wrapping() {
        assert_eq!(pdf_date("2026-11-03T14:05:09.123Z"), "D:20261103140509Z");
        assert_eq!(
            wrap("one two three four", 9),
            vec!["one two", "three", "four"]
        );
        let q = quads_obj(&[[1.0, 2.0, 3.0, 4.0]]);
        let v: Vec<f32> = q
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o.as_float().unwrap())
            .collect();
        assert_eq!(v, vec![1.0, 4.0, 3.0, 4.0, 1.0, 2.0, 3.0, 2.0]);
    }
}
