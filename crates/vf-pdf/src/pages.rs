//! Page edits within one document (P04 step 6): rotate, delete, reorder. Combining documents
//! and cutting page ranges out reuse Filer's `merge_pdfs` / `split_pdf` (vendored, with
//! bookmarks per source and form fields kept).
//!
//! Before reordering, every page gets the attributes it inherits from the page tree
//! (Resources, MediaBox, CropBox, Rotate) and hangs directly under the root, so moving a page
//! never changes how it looks.

use std::path::Path;

use lopdf::{Document, Object, ObjectId};

fn load(src: &Path) -> Result<Document, String> {
    let doc = Document::load(src).map_err(|e| format!("cannot read the PDF: {e}"))?;
    if doc.is_encrypted() {
        return Err("the PDF is password protected".into());
    }
    Ok(doc)
}

fn save(doc: &mut Document, out: &Path) -> Result<(), String> {
    doc.prune_objects();
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)
        .map_err(|e| format!("cannot write the PDF: {e}"))?;
    std::fs::write(out, bytes).map_err(|e| format!("cannot write {}: {e}", out.display()))
}

/// Turns `pages` (0-based) by `degrees` (a multiple of 90; negative = left).
pub fn rotate(src: &Path, out: &Path, pages: &[usize], degrees: i64) -> Result<(), String> {
    if degrees % 90 != 0 {
        return Err("pages turn in steps of 90°".into());
    }
    let mut doc = load(src)?;
    let ids: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    for &p in pages {
        let id = *ids
            .get(p)
            .ok_or_else(|| format!("page {} is not in the PDF", p + 1))?;
        let now = crate::layer::page_rotate(&doc, id);
        doc.get_dictionary_mut(id)
            .map_err(|e| e.to_string())?
            .set("Rotate", (now + degrees).rem_euclid(360));
    }
    save(&mut doc, out)
}

/// Removes `pages` (0-based). At least one page must stay.
pub fn delete(src: &Path, out: &Path, pages: &[usize]) -> Result<(), String> {
    let mut doc = load(src)?;
    let n = doc.get_pages().len();
    let mut gone: Vec<usize> = pages.iter().copied().filter(|&p| p < n).collect();
    gone.sort_unstable();
    gone.dedup();
    if gone.len() >= n {
        return Err("a document needs at least one page".into());
    }
    let order: Vec<usize> = (0..n).filter(|p| !gone.contains(p)).collect();
    set_order(&mut doc, &order)?;
    save(&mut doc, out)
}

/// Puts the pages in `order` (0-based indices of the current pages; each exactly once).
pub fn reorder(src: &Path, out: &Path, order: &[usize]) -> Result<(), String> {
    let mut doc = load(src)?;
    let n = doc.get_pages().len();
    let mut seen = order.to_vec();
    seen.sort_unstable();
    if seen != (0..n).collect::<Vec<_>>() {
        return Err(format!(
            "the new order must list each of the {n} pages once"
        ));
    }
    set_order(&mut doc, order)?;
    save(&mut doc, out)
}

/// The root page tree holds exactly `order` (current page indices), each page carrying its
/// inherited attributes; pages left out are dropped.
fn set_order(doc: &mut Document, order: &[usize]) -> Result<(), String> {
    let ids: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    let root = doc
        .catalog()
        .map_err(|e| e.to_string())?
        .get(b"Pages")
        .and_then(Object::as_reference)
        .map_err(|e| format!("no page tree: {e}"))?;
    for &id in &ids {
        for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
            let has = doc.get_dictionary(id).map(|d| d.has(key)).unwrap_or(true);
            if !has && let Some(v) = inherited(doc, id, key) {
                doc.get_dictionary_mut(id)
                    .map_err(|e| e.to_string())?
                    .set(key, v);
            }
        }
    }
    let kids: Vec<Object> = order
        .iter()
        .map(|&i| {
            ids.get(i)
                .map(|id| Object::Reference(*id))
                .ok_or_else(|| format!("page {} is not in the PDF", i + 1))
        })
        .collect::<Result<_, _>>()?;
    for &i in order {
        doc.get_dictionary_mut(ids[i])
            .map_err(|e| e.to_string())?
            .set("Parent", Object::Reference(root));
    }
    let tree = doc.get_dictionary_mut(root).map_err(|e| e.to_string())?;
    tree.set("Count", kids.len() as i64);
    tree.set("Kids", kids);
    Ok(())
}

fn inherited(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut id = doc
        .get_dictionary(page)
        .ok()?
        .get(b"Parent")
        .ok()?
        .as_reference()
        .ok()?;
    for _ in 0..32 {
        let d = doc.get_dictionary(id).ok()?;
        if let Ok(v) = d.get(key) {
            return Some(v.clone());
        }
        id = d.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

/// "1-3, 5, 8-9" → 0-based page indices (in the order written, duplicates dropped), checked
/// against `count` pages.
pub fn parse_ranges(text: &str, count: usize) -> Result<Vec<usize>, String> {
    let mut out: Vec<usize> = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (part, part),
        };
        let a: usize = a
            .parse()
            .map_err(|_| format!("\"{part}\" is not a page or range"))?;
        let b: usize = b
            .parse()
            .map_err(|_| format!("\"{part}\" is not a page or range"))?;
        if a == 0 || b == 0 || a > count || b > count {
            return Err(format!("pages run 1 to {count}"));
        }
        let range: Vec<usize> = if a <= b {
            (a..=b).collect()
        } else {
            (b..=a).rev().collect()
        };
        for p in range {
            if !out.contains(&(p - 1)) {
                out.push(p - 1);
            }
        }
    }
    if out.is_empty() {
        return Err("no pages given".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_ranges("1-3, 5", 6).unwrap(), [0, 1, 2, 4]);
        assert_eq!(parse_ranges("3-1", 3).unwrap(), [2, 1, 0]);
        assert_eq!(parse_ranges("2,2,2", 3).unwrap(), [1]);
        assert!(parse_ranges("0", 3).is_err());
        assert!(parse_ranges("4", 3).is_err());
        assert!(parse_ranges("x", 3).is_err());
        assert!(parse_ranges(" ", 3).is_err());
    }
}
