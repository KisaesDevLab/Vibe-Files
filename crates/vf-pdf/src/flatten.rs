//! Flatten (P04 step 4): every annotation and form field with an appearance becomes part of
//! the page content, then the annotation is removed, so the page looks the same everywhere and
//! nothing can be edited, hidden or moved. Links stay (they have no appearance to burn in).
//! Hidden annotations and popups are dropped.

use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

/// Flattens `src` into `out`. Returns how many annotations were burnt in.
pub fn flatten(src: &Path, out: &Path) -> Result<usize, String> {
    let mut doc = Document::load(src).map_err(|e| format!("cannot read the PDF: {e}"))?;
    if doc.is_encrypted() {
        return Err("the PDF is password protected".into());
    }
    let n = flatten_doc(&mut doc)?;
    doc.prune_objects();
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)
        .map_err(|e| format!("cannot write the PDF: {e}"))?;
    std::fs::write(out, bytes).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(n)
}

/// Flattens in place (PDF/A uses this before its own checks).
pub fn flatten_doc(doc: &mut Document) -> Result<usize, String> {
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    let mut total = 0;
    for page in pages {
        let annots: Vec<Object> = match doc
            .get_dictionary(page)
            .ok()
            .and_then(|d| d.get(b"Annots").ok().cloned())
        {
            Some(o) => match doc.dereference(&o) {
                Ok((_, Object::Array(a))) => a.clone(),
                _ => continue,
            },
            None => continue,
        };
        let mut keep = Vec::new();
        let mut draws: Vec<(ObjectId, [f32; 6])> = Vec::new();
        for a in annots {
            let Ok((_, Object::Dictionary(d))) = doc.dereference(&a) else {
                continue;
            };
            let d = d.clone();
            let subtype = d
                .get(b"Subtype")
                .ok()
                .and_then(|o| o.as_name().ok())
                .map(<[u8]>::to_vec)
                .unwrap_or_default();
            if subtype == b"Link" {
                keep.push(a);
                continue;
            }
            let flags = d.get(b"F").ok().and_then(|o| o.as_i64().ok()).unwrap_or(0);
            if subtype == b"Popup" || flags & 2 != 0 {
                continue; // hidden or a popup: dropped
            }
            let Some(ap) = appearance(doc, &d) else {
                continue; // nothing to draw (no appearance): dropped
            };
            let Some(rect) = rect(&d) else { continue };
            if let Some(m) = placement(doc, ap, rect) {
                draws.push((ap, m));
                total += 1;
            }
        }
        if draws.is_empty() && keep.is_empty() {
            if let Ok(p) = doc.get_dictionary_mut(page) {
                p.remove(b"Annots");
            }
            continue;
        }
        if !draws.is_empty() {
            burn_in(doc, page, &draws)?;
        }
        let p = doc.get_dictionary_mut(page).map_err(|e| e.to_string())?;
        if keep.is_empty() {
            p.remove(b"Annots");
        } else {
            p.set("Annots", keep);
        }
    }
    // Form fields are part of the pages now.
    if let Ok(cat) = doc.catalog_mut() {
        cat.remove(b"AcroForm");
    }
    Ok(total)
}

/// The normal appearance stream of an annotation (for a field with states, the one `/AS`
/// names).
fn appearance(doc: &Document, d: &Dictionary) -> Option<ObjectId> {
    let ap = d.get(b"AP").ok()?;
    let (_, ap) = doc.dereference(ap).ok()?;
    let n = ap.as_dict().ok()?.get(b"N").ok()?;
    match n {
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Stream(_) => Some(*id),
            Object::Dictionary(states) => state(states, d),
            _ => None,
        },
        Object::Dictionary(states) => state(states, d),
        _ => None,
    }
}

fn state(states: &Dictionary, d: &Dictionary) -> Option<ObjectId> {
    let on = d.get(b"AS").ok()?.as_name().ok()?;
    states.get(on).ok()?.as_reference().ok()
}

fn rect(d: &Dictionary) -> Option<[f32; 4]> {
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

/// The matrix that puts the appearance's (transformed) BBox onto the annotation Rect
/// (ISO 32000-1 §12.5.5); the form's own `/Matrix` is applied by `Do`.
fn placement(doc: &Document, ap: ObjectId, r: [f32; 4]) -> Option<[f32; 6]> {
    let Object::Stream(s) = doc.get_object(ap).ok()? else {
        return None;
    };
    let nums = |k: &[u8], n: usize| -> Option<Vec<f32>> {
        let v: Vec<f32> = s
            .dict
            .get(k)
            .ok()?
            .as_array()
            .ok()?
            .iter()
            .filter_map(|o| o.as_float().ok())
            .collect();
        (v.len() == n).then_some(v)
    };
    let b = nums(b"BBox", 4)?;
    let m = nums(b"Matrix", 6).unwrap_or_else(|| vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let corners = [[b[0], b[1]], [b[2], b[1]], [b[0], b[3]], [b[2], b[3]]]
        .map(|[x, y]| [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]);
    let (x0, x1) = corners
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), c| (a.min(c[0]), b.max(c[0])));
    let (y0, y1) = corners
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), c| (a.min(c[1]), b.max(c[1])));
    if x1 - x0 <= 0.0 || y1 - y0 <= 0.0 {
        return None;
    }
    let sx = (r[2] - r[0]) / (x1 - x0);
    let sy = (r[3] - r[1]) / (y1 - y0);
    Some([sx, 0.0, 0.0, sy, r[0] - x0 * sx, r[1] - y0 * sy])
}

/// `q <page content> Q` then each appearance drawn as a form XObject at its place.
fn burn_in(
    doc: &mut Document,
    page: ObjectId,
    draws: &[(ObjectId, [f32; 6])],
) -> Result<(), String> {
    // Appearance streams become form XObjects (they are already `/Subtype /Form` or close).
    let mut names = Vec::new();
    for (i, (ap, _)) in draws.iter().enumerate() {
        if let Ok(Object::Stream(s)) = doc.get_object_mut(*ap) {
            s.dict.set("Type", Object::Name(b"XObject".to_vec()));
            s.dict.set("Subtype", Object::Name(b"Form".to_vec()));
        }
        names.push(format!("VfFlat{i}"));
    }
    // Resources: copy inherited ones down, then add the forms.
    let own = doc
        .get_dictionary(page)
        .map_err(|e| e.to_string())?
        .has(b"Resources");
    if !own {
        let inherited = crate::layer::inherited_resources(doc, page);
        doc.get_dictionary_mut(page)
            .map_err(|e| e.to_string())?
            .set("Resources", inherited);
    }
    let res = doc
        .get_or_create_resources(page)
        .and_then(Object::as_dict_mut)
        .map_err(|e| e.to_string())?;
    let xobj_ref = match res.get(b"XObject") {
        Ok(Object::Reference(id)) => Some(*id),
        _ => None,
    };
    let xobjects = match xobj_ref {
        Some(id) => doc.get_dictionary_mut(id).map_err(|e| e.to_string())?,
        None => {
            let res = doc
                .get_or_create_resources(page)
                .and_then(Object::as_dict_mut)
                .map_err(|e| e.to_string())?;
            if !matches!(res.get(b"XObject"), Ok(Object::Dictionary(_))) {
                res.set("XObject", Dictionary::new());
            }
            res.get_mut(b"XObject")
                .and_then(Object::as_dict_mut)
                .map_err(|e| e.to_string())?
        }
    };
    for (name, (ap, _)) in names.iter().zip(draws) {
        xobjects.set(name.as_bytes(), Object::Reference(*ap));
    }
    let mut ops = String::from("Q\n");
    for (name, (_, m)) in names.iter().zip(draws) {
        ops.push_str(&format!(
            "q {} {} {} {} {} {} cm /{name} Do Q\n",
            m[0], m[1], m[2], m[3], m[4], m[5]
        ));
    }
    let existing: Vec<Object> = match doc
        .get_dictionary(page)
        .map_err(|e| e.to_string())?
        .get(b"Contents")
    {
        Ok(Object::Reference(id)) => vec![Object::Reference(*id)],
        Ok(Object::Array(a)) => a.clone(),
        _ => vec![],
    };
    let push = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let tail = doc.add_object(Stream::new(Dictionary::new(), ops.into_bytes()));
    let mut contents = vec![Object::Reference(push)];
    contents.extend(existing);
    contents.push(Object::Reference(tail));
    doc.get_dictionary_mut(page)
        .map_err(|e| e.to_string())?
        .set("Contents", contents);
    Ok(())
}
