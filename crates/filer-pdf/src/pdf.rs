//! PDF support: locating and binding pdfium, first-page text for doc-type sniffing (P06),
//! merging (P08), page texts, thumbnails and splitting (P09).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};

use pdfium_render::prelude::{Pdfium, PdfiumError};

/// Returns the folder holding a `pdfium.dll` that binds, or `None` (PDF features disabled).
/// Looks next to the exe; debug builds also look in the repo's `assets/`.
pub fn find_pdfium(exe_dir: &Path) -> Option<PathBuf> {
    let mut dirs = vec![exe_dir.to_path_buf()];
    if cfg!(debug_assertions) {
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets"));
    }
    dirs.into_iter().find(|dir| {
        let lib = Pdfium::pdfium_platform_library_name_at_path(dir);
        // Once pdfium is in use (the process-wide instance), binding again reports
        // "already initialized": that still means it works.
        lib.is_file()
            && match Pdfium::bind_to_library(&lib) {
                Ok(_) => true,
                Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => true,
                Err(_) => false,
            }
    })
}

/// Combined (and later split) PDFs are built in `data/tmp/<id>/` until they are filed.
pub const TMP_DIR: &str = "tmp";

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

/// The process-wide pdfium instance (pdfium-render allows binding the library only once).
pub fn pdfium(dir: &Path) -> Result<&'static Pdfium, String> {
    PDFIUM
        .get_or_init(|| {
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(dir))
                .map(Pdfium::new)
                .map_err(|e| format!("cannot load pdfium: {e}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// Text of the first page (empty when the page has no text layer, e.g. a scan).
pub fn first_page_text(pdfium: &Pdfium, path: &Path) -> Result<String, String> {
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| format!("cannot open PDF: {e}"))?;
    let page = doc
        .pages()
        .get(0)
        .map_err(|e| format!("PDF has no first page: {e}"))?;
    let text = page.text().map_err(|e| format!("cannot read text: {e}"))?;
    Ok(text.all())
}

/// First-page text for one file, as far as it is known.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PageText {
    Pending,
    Text(String),
    Failed(String),
}

/// Reads first-page text on one worker thread, one file at a time, and caches the results.
pub struct Sniffer {
    tx: Option<Sender<PathBuf>>,
    rx: Option<Receiver<(PathBuf, PageText)>>,
    results: HashMap<PathBuf, PageText>,
    /// Bumped whenever `results` changes, so Preview can tell without hashing every text.
    generation: u64,
}

impl Sniffer {
    /// `pdfium_dir` `None`: PDF features are off and nothing is ever read.
    pub fn new(ctx: &egui::Context, pdfium_dir: Option<PathBuf>) -> Self {
        let Some(dir) = pdfium_dir else {
            return Self::disabled();
        };
        let (tx, jobs) = channel::<PathBuf>();
        let (out, rx) = channel();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("pdf-text".into())
            .spawn(move || {
                for path in jobs {
                    let result = pdfium(&dir)
                        .and_then(|p| first_page_text(p, &path))
                        .map_or_else(PageText::Failed, PageText::Text);
                    if out.send((path, result)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn pdf-text thread");
        Self {
            tx: Some(tx),
            rx: Some(rx),
            results: HashMap::new(),
            generation: 0,
        }
    }

    pub fn enabled(&self) -> bool {
        self.tx.is_some()
    }

    /// Never reads anything (PDF features off, or before the UI exists).
    pub fn disabled() -> Self {
        Self {
            tx: None,
            rx: None,
            results: HashMap::new(),
            generation: 0,
        }
    }

    /// Queues `path` for reading unless it was already read or requested.
    pub fn request(&mut self, path: &Path) {
        let Some(tx) = &self.tx else { return };
        if self.results.contains_key(path) {
            return;
        }
        self.results.insert(path.to_path_buf(), PageText::Pending);
        self.generation += 1;
        let _ = tx.send(path.to_path_buf());
    }

    /// Collects finished reads. Call every frame.
    pub fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            for (path, text) in rx.try_iter() {
                if let PageText::Failed(e) = &text {
                    tracing::info!(file = %path.display(), error = %e, "no first-page text");
                }
                self.results.insert(path, text);
                self.generation += 1;
            }
        }
    }

    /// Changes whenever a file is requested or its text arrives.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn get(&self, path: &Path) -> Option<&PageText> {
        self.results.get(path)
    }
}

/// Number of pages in a PDF.
pub fn page_count(path: &Path) -> Result<usize, String> {
    lopdf::Document::load(path)
        .map(|d| d.get_pages().len())
        .map_err(|e| format!("cannot open {}: {e}", path.display()))
}

/// Page attributes a page may inherit from its page-tree ancestors (PDF 32000 §7.7.3.4).
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// The value of `key` for `page`, from the page itself or the nearest ancestor that has it.
fn inherited(doc: &lopdf::Document, page: lopdf::ObjectId, key: &[u8]) -> Option<lopdf::Object> {
    let mut dict = doc.get_dictionary(page).ok()?;
    for _ in 0..64 {
        if let Ok(v) = dict.get(key) {
            return Some(v.clone());
        }
        let parent = dict
            .get(b"Parent")
            .and_then(lopdf::Object::as_reference)
            .ok()?;
        dict = doc.get_dictionary(parent).ok()?;
    }
    None
}

/// Merges `inputs` (path, bookmark title) in order into `out`, with one bookmark per input at
/// its first page (X87). Returns the page count. `out` must not exist.
///
/// Each page gets the attributes it inherited from its own page tree (Resources, MediaBox,
/// CropBox, Rotate), because the merged file has a single page tree. Fillable-form fields are
/// kept: all inputs' fields go into one AcroForm, and fields of the 2nd and later inputs are
/// renamed `docN_<name>` so same-named fields don't share values. Other catalog entries of
/// inputs after the first (named destinations, structure tree) are dropped.
pub fn merge_pdfs(inputs: &[(PathBuf, String)], out: &Path) -> Result<usize, String> {
    use lopdf::{Bookmark, Document, Object, ObjectId};
    use std::collections::BTreeMap;

    let mut merged = Document::with_version("1.5");
    let mut max_id = 1;
    // Pages in reading order (the lopdf example keys them by object id, which reorders them).
    let mut pages: Vec<(ObjectId, Object)> = Vec::new();
    let mut objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut form_fields: Vec<Object> = Vec::new();
    let mut form_defaults: Option<(Option<Object>, Option<Object>)> = None;
    for (index, (path, title)) in inputs.iter().enumerate() {
        let mut doc =
            Document::load(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        if doc.is_encrypted() {
            return Err(format!("{} is password-protected", path.display()));
        }
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;
        collect_form_fields(&mut doc, index, &mut form_fields, &mut form_defaults);
        let doc_pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
        let Some(first) = doc_pages.first() else {
            return Err(format!("{} has no pages", path.display()));
        };
        merged.add_bookmark(
            Bookmark::new(title.clone(), [0.0, 0.0, 0.0], 0, *first),
            None,
        );
        for id in doc_pages {
            let mut page = doc
                .get_object(id)
                .map_err(|e| format!("{}: bad page: {e}", path.display()))?
                .to_owned();
            if let Object::Dictionary(dict) = &mut page {
                for key in INHERITABLE {
                    if !dict.has(key)
                        && let Some(value) = inherited(&doc, id, key)
                    {
                        dict.set(key.to_vec(), value);
                    }
                }
            }
            pages.push((id, page));
        }
        objects.extend(doc.objects);
    }

    let mut catalog: Option<(ObjectId, Object)> = None;
    let mut pages_root: Option<(ObjectId, Object)> = None;
    for (id, object) in objects {
        match object.type_name().unwrap_or(b"") {
            b"Catalog" => {
                catalog = Some((catalog.map_or(id, |(c, _)| c), object));
            }
            b"Pages" => {
                if let Ok(dict) = object.as_dict() {
                    let mut dict = dict.clone();
                    if let Some((_, old)) = &pages_root
                        && let Ok(old) = old.as_dict()
                    {
                        dict.extend(old);
                    }
                    pages_root =
                        Some((pages_root.map_or(id, |(p, _)| p), Object::Dictionary(dict)));
                }
            }
            b"Page" | b"Outlines" | b"Outline" => {}
            _ => {
                merged.objects.insert(id, object);
            }
        }
    }
    let (pages_id, pages_obj) = pages_root.ok_or("no page tree found")?;
    let (catalog_id, catalog_obj) = catalog.ok_or("no catalog found")?;

    for (id, page) in &pages {
        if let Ok(dict) = page.as_dict() {
            let mut dict = dict.clone();
            dict.set("Parent", pages_id);
            merged.objects.insert(*id, Object::Dictionary(dict));
        }
    }
    let mut pages_dict = pages_obj.as_dict().map_err(|e| e.to_string())?.clone();
    pages_dict.set("Count", pages.len() as u32);
    pages_dict.set(
        "Kids",
        pages
            .iter()
            .map(|(id, _)| Object::Reference(*id))
            .collect::<Vec<_>>(),
    );
    merged
        .objects
        .insert(pages_id, Object::Dictionary(pages_dict));

    let mut catalog_dict = catalog_obj.as_dict().map_err(|e| e.to_string())?.clone();
    catalog_dict.set("Pages", pages_id);
    catalog_dict.set("PageMode", "UseOutlines");
    catalog_dict.remove(b"Outlines");
    catalog_dict.remove(b"AcroForm");
    if !form_fields.is_empty() {
        let mut form = lopdf::Dictionary::new();
        form.set("Fields", form_fields);
        form.set("NeedAppearances", true);
        if let Some((dr, da)) = form_defaults {
            if let Some(dr) = dr {
                form.set("DR", dr);
            }
            if let Some(da) = da {
                form.set("DA", da);
            }
        }
        catalog_dict.set("AcroForm", form);
    }
    merged
        .objects
        .insert(catalog_id, Object::Dictionary(catalog_dict));
    merged.trailer.set("Root", catalog_id);

    merged.max_id = merged.objects.len() as u32;
    merged.renumber_objects();
    merged.adjust_zero_pages();
    if let Some(outline) = merged.build_outline() {
        // Renumbering may have moved the catalog; find it through the trailer.
        let root = merged
            .trailer
            .get(b"Root")
            .and_then(Object::as_reference)
            .map_err(|e| format!("merged PDF has no root: {e}"))?;
        if let Ok(Object::Dictionary(dict)) = merged.get_object_mut(root) {
            dict.set("Outlines", Object::Reference(outline));
        }
    }
    merged.compress();

    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    merged
        .save_to(&mut writer)
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    use std::io::Write;
    writer
        .flush()
        .and_then(|()| writer.get_ref().sync_all())
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(pages.len())
}

/// Adds `doc`'s AcroForm fields to `fields` (renaming top-level fields of inputs after the
/// first) and remembers the first form's default resources and appearance.
fn collect_form_fields(
    doc: &mut lopdf::Document,
    index: usize,
    fields: &mut Vec<lopdf::Object>,
    defaults: &mut Option<(Option<lopdf::Object>, Option<lopdf::Object>)>,
) {
    use lopdf::Object;
    let Ok(catalog) = doc.catalog() else { return };
    let Ok(form_obj) = catalog.get(b"AcroForm") else {
        return;
    };
    let Ok((_, form)) = doc.dereference(form_obj) else {
        return;
    };
    let Ok(form) = form.as_dict() else { return };
    if defaults.is_none() {
        *defaults = Some((form.get(b"DR").ok().cloned(), form.get(b"DA").ok().cloned()));
    }
    let refs: Vec<Object> = form
        .get(b"Fields")
        .and_then(Object::as_array)
        .cloned()
        .unwrap_or_default();
    for field in refs {
        if index > 0
            && let Ok(id) = field.as_reference()
            && let Ok(Object::Dictionary(dict)) = doc.get_object_mut(id)
            && let Ok(Object::String(name, format)) = dict.get(b"T").cloned()
        {
            let mut renamed = format!("doc{}_", index + 1).into_bytes();
            renamed.extend_from_slice(&name);
            dict.set("T", Object::String(renamed, format));
        }
        fields.push(field);
    }
}

/// Text of every page, in order (empty strings for pages without a text layer).
pub fn page_texts(pdfium: &Pdfium, path: &Path) -> Result<Vec<String>, String> {
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| format!("cannot open PDF: {e}"))?;
    doc.pages()
        .iter()
        .map(|p| {
            p.text()
                .map(|t| t.all())
                .map_err(|e| format!("cannot read text: {e}"))
        })
        .collect()
}

/// Renders one page (0-based) of an open document `width` pixels wide.
fn render_doc_page(
    doc: &pdfium_render::prelude::PdfDocument<'_>,
    page: usize,
    width: u32,
) -> Result<egui::ColorImage, String> {
    use pdfium_render::prelude::PdfRenderConfig;
    let index = i32::try_from(page).map_err(|_| "page number too large".to_string())?;
    let p = doc
        .pages()
        .get(index)
        .map_err(|e| format!("no page {}: {e}", page + 1))?;
    let config = PdfRenderConfig::new().set_target_width(width as i32);
    let image = p
        .render_with_config(&config)
        .and_then(|b| b.as_image())
        .map_err(|e| format!("cannot render page {}: {e}", page + 1))?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        size,
        image.as_raw(),
    ))
}

/// Renders one page (0-based) `width` pixels wide.
#[cfg(any(test, feature = "test-support"))]
pub fn render_page(
    pdfium: &Pdfium,
    path: &Path,
    page: usize,
    width: u32,
) -> Result<egui::ColorImage, String> {
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| format!("cannot open PDF: {e}"))?;
    render_doc_page(&doc, page, width)
}

/// Writes one PDF per page range (0-based, end exclusive) of `path`, to the matching `outs`
/// (which must not exist). Each piece's pages hang directly under the page-tree root, with the
/// attributes they inherited copied onto them. Only form fields whose widgets are on the piece's
/// pages are kept; the outline, structure tree, names and open action are dropped, so no other
/// client's data stays reachable in a piece. On any error, pieces already written are removed.
pub fn split_pdf(
    path: &Path,
    ranges: &[std::ops::Range<usize>],
    outs: &[PathBuf],
) -> Result<(), String> {
    use lopdf::{Object, ObjectId};
    if ranges.len() != outs.len() {
        return Err("internal error: ranges and outputs differ".into());
    }
    let source =
        lopdf::Document::load(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    if source.is_encrypted() {
        return Err(format!("{} is password-protected", path.display()));
    }
    let page_ids: Vec<ObjectId> = source.get_pages().into_values().collect();
    for range in ranges {
        if range.is_empty() || range.end > page_ids.len() {
            return Err(format!(
                "pages {}-{} are not in the PDF",
                range.start + 1,
                range.end
            ));
        }
    }
    let root = source
        .catalog()
        .and_then(|c| c.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(|e| format!("{} has no page tree: {e}", path.display()))?;

    let mut written: Vec<&PathBuf> = Vec::new();
    let result = (|| -> Result<(), String> {
        for (range, out) in ranges.iter().zip(outs) {
            let mut doc = source.clone();
            let kept: Vec<ObjectId> = page_ids[range.clone()].to_vec();
            for &id in &kept {
                let attrs: Vec<(&[u8], Object)> = INHERITABLE
                    .iter()
                    .filter(|k| doc.get_dictionary(id).is_ok_and(|d| !d.has(k)))
                    .filter_map(|k| inherited(&doc, id, k).map(|v| (*k, v)))
                    .collect();
                if let Ok(Object::Dictionary(d)) = doc.get_object_mut(id) {
                    for (k, v) in attrs {
                        d.set(k.to_vec(), v);
                    }
                    d.set("Parent", root);
                }
            }
            if let Ok(Object::Dictionary(d)) = doc.get_object_mut(root) {
                d.set(
                    "Kids",
                    kept.iter()
                        .map(|id| Object::Reference(*id))
                        .collect::<Vec<_>>(),
                );
                d.set("Count", kept.len() as i64);
                d.remove(b"Parent");
            }
            keep_fields_on_pages(&mut doc, &kept);
            if let Ok(catalog) = doc.catalog_mut() {
                for key in [
                    &b"Outlines"[..],
                    b"StructTreeRoot",
                    b"Names",
                    b"Dests",
                    b"OpenAction",
                ] {
                    catalog.remove(key);
                }
            }
            doc.prune_objects();
            doc.compress();
            written.push(out);
            save_new(&mut doc, out)?;
        }
        Ok(())
    })();
    if result.is_err() {
        // Our own partial output only.
        for out in written {
            let _ = std::fs::remove_file(out);
        }
    }
    result
}

/// Saves `doc` to `out`, which must not exist yet.
pub(crate) fn save_new(doc: &mut lopdf::Document, out: &Path) -> Result<(), String> {
    use std::io::Write;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    doc.save_to(&mut writer)
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    writer
        .flush()
        .and_then(|()| writer.get_ref().sync_all())
        .map_err(|e| format!("cannot write {}: {e}", out.display()))
}

/// Keeps only the AcroForm fields that have a widget on one of `pages`.
fn keep_fields_on_pages(doc: &mut lopdf::Document, pages: &[lopdf::ObjectId]) {
    use lopdf::{Object, ObjectId};
    use std::collections::HashSet;
    let mut widgets: HashSet<ObjectId> = HashSet::new();
    for &p in pages {
        let Ok(page) = doc.get_dictionary(p) else {
            continue;
        };
        let Ok(annots) = page.get(b"Annots") else {
            continue;
        };
        let Ok((_, annots)) = doc.dereference(annots) else {
            continue;
        };
        if let Ok(list) = annots.as_array() {
            widgets.extend(list.iter().filter_map(|a| a.as_reference().ok()));
        }
    }
    fn on_pages(
        doc: &lopdf::Document,
        id: ObjectId,
        widgets: &HashSet<ObjectId>,
        depth: u8,
    ) -> bool {
        if widgets.contains(&id) {
            return true;
        }
        if depth > 16 {
            return false;
        }
        doc.get_dictionary(id)
            .and_then(|d| d.get(b"Kids"))
            .and_then(Object::as_array)
            .is_ok_and(|kids| {
                kids.iter()
                    .filter_map(|k| k.as_reference().ok())
                    .any(|k| on_pages(doc, k, widgets, depth + 1))
            })
    }
    let Ok(catalog) = doc.catalog() else { return };
    let Ok(form_obj) = catalog.get(b"AcroForm") else {
        return;
    };
    let Ok((_, form)) = doc.dereference(form_obj) else {
        return;
    };
    let Ok(form) = form.as_dict() else { return };
    let mut form = form.clone();
    let fields: Vec<Object> = form
        .get(b"Fields")
        .and_then(Object::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|f| {
            f.as_reference()
                .is_ok_and(|id| on_pages(doc, id, &widgets, 0))
        })
        .collect();
    let empty = fields.is_empty();
    form.set("Fields", fields);
    if let Ok(catalog) = doc.catalog_mut() {
        if empty {
            catalog.remove(b"AcroForm");
        } else {
            catalog.set("AcroForm", form);
        }
    }
}

/// Renders thumbnails on one worker thread, one page at a time, from the document opened once.
pub struct Thumbnailer {
    tx: Sender<usize>,
    rx: Receiver<(usize, Result<egui::ColorImage, String>)>,
}

impl Thumbnailer {
    pub fn new(ctx: &egui::Context, pdfium_dir: PathBuf, path: PathBuf, width: u32) -> Self {
        let (tx, jobs) = channel::<usize>();
        let (out, rx) = channel();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("thumbnails".into())
            .spawn(move || {
                let doc = pdfium(&pdfium_dir).and_then(|p| {
                    p.load_pdf_from_file(&path, None)
                        .map_err(|e| format!("cannot open PDF: {e}"))
                });
                for page in jobs {
                    let image = match &doc {
                        Ok(doc) => render_doc_page(doc, page, width),
                        Err(e) => Err(e.clone()),
                    };
                    if out.send((page, image)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn thumbnails thread");
        Self { tx, rx }
    }

    pub fn request(&self, page: usize) {
        let _ = self.tx.send(page);
    }

    pub fn finished(&self) -> Vec<(usize, Result<egui::ColorImage, String>)> {
        self.rx.try_iter().collect()
    }
}

/// Writes a simple PDF with one text line per page (test fixtures).
#[cfg(any(test, feature = "test-support"))]
pub fn write_test_pdf(path: &Path, pages: &[&str]) {
    use lopdf::content::{Content, Operation};
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
    let mut kids: Vec<Object> = Vec::new();
    for text in pages {
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 18.into()]),
                Operation::new("Td", vec![72.into(), 700.into()]),
                Operation::new("Tj", vec![Object::string_literal(*text)]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        kids.push(page_id.into());
    }
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => kids, "Count" => count, "Resources" => resources_id,
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> PathBuf {
        find_pdfium(Path::new("")).expect("assets/pdfium.dll binds")
    }

    #[test]
    fn reads_first_page_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.pdf");
        write_test_pdf(&path, &["Engagement Letter for tax year 2025", "page two"]);
        let p = pdfium(&assets()).unwrap();
        let text = first_page_text(p, &path).unwrap();
        assert!(text.contains("Engagement Letter"), "{text:?}");
        assert!(!text.contains("page two"));

        let bad = dir.path().join("bad.pdf");
        std::fs::write(&bad, b"not a pdf").unwrap();
        assert!(first_page_text(p, &bad).is_err());
    }

    #[test]
    fn merges_in_order_with_bookmarks() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.pdf");
        let b = dir.path().join("b.pdf");
        let c = dir.path().join("c.pdf");
        write_test_pdf(&a, &["A1"]);
        write_test_pdf(&b, &["B1", "B2"]);
        write_test_pdf(&c, &["C1", "C2", "C3"]);
        let out = dir.path().join("out.pdf");
        let inputs = vec![
            (c.clone(), "c.pdf".to_string()),
            (a.clone(), "a.pdf".to_string()),
            (b.clone(), "b.pdf".to_string()),
        ];
        assert_eq!(merge_pdfs(&inputs, &out).unwrap(), 6);
        assert_eq!(page_count(&out).unwrap(), 6);

        // Page order follows the input order, read back with pdfium.
        let p = pdfium(&assets()).unwrap();
        let doc = p.load_pdf_from_file(&out, None).unwrap();
        let texts: Vec<String> = doc
            .pages()
            .iter()
            .map(|pg| pg.text().unwrap().all().trim().to_string())
            .collect();
        assert_eq!(texts, ["C1", "C2", "C3", "A1", "B1", "B2"]);
        let titles: Vec<String> = doc.bookmarks().iter().filter_map(|b| b.title()).collect();
        assert_eq!(titles, ["c.pdf", "a.pdf", "b.pdf"]);
        drop(doc);

        // Never overwrites, and bad input fails cleanly.
        assert!(merge_pdfs(&inputs, &out).is_err());
        let bad = dir.path().join("bad.pdf");
        std::fs::write(&bad, b"nope").unwrap();
        let out2 = dir.path().join("out2.pdf");
        assert!(merge_pdfs(&[(bad, "bad".into())], &out2).is_err());
        assert!(!out2.exists());
    }

    /// One page whose MediaBox, Rotate and Resources live on the page-tree node only, plus an
    /// optional fillable text field named `field`.
    fn write_inheriting_pdf(
        path: &Path,
        text: &str,
        height: i64,
        rotate: i64,
        field: Option<(&str, &str)>,
    ) {
        use lopdf::content::{Content, Operation};
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier",
        });
        let resources_id =
            doc.add_object(dictionary! { "Font" => dictionary! { "F2" => font_id } });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F2".into(), 18.into()]),
                Operation::new("Td", vec![72.into(), 500.into()]),
                Operation::new("Tj", vec![Object::string_literal(text)]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.new_object_id();
        let mut page =
            dictionary! { "Type" => "Page", "Parent" => pages_id, "Contents" => content_id };
        let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
        if let Some((name, value)) = field {
            let widget = doc.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx",
                "T" => Object::string_literal(name), "V" => Object::string_literal(value),
                "Rect" => vec![72.into(), 600.into(), 300.into(), 620.into()], "P" => page_id,
            });
            page.set("Annots", vec![widget.into()]);
            catalog.set("AcroForm", dictionary! { "Fields" => vec![widget.into()] });
        }
        doc.objects.insert(page_id, Object::Dictionary(page));
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
                "Resources" => resources_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), height.into()],
                "Rotate" => rotate,
            }),
        );
        let catalog_id = doc.add_object(catalog);
        doc.trailer.set("Root", catalog_id);
        doc.save(path).unwrap();
    }

    #[test]
    fn merged_pages_keep_their_inherited_size_rotation_and_fonts() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("letter.pdf"), dir.path().join("legal.pdf"));
        write_inheriting_pdf(&a, "LETTER", 792, 0, None);
        write_inheriting_pdf(&b, "LEGAL ROTATED", 1008, 90, None);
        let out = dir.path().join("out.pdf");
        merge_pdfs(&[(a, "a".into()), (b, "b".into())], &out).unwrap();

        let doc = lopdf::Document::load(&out).unwrap();
        let pages: Vec<_> = doc.get_pages().into_values().collect();
        let height = |id| match inherited(&doc, id, b"MediaBox") {
            Some(lopdf::Object::Array(a)) => a[3].as_i64().unwrap(),
            other => panic!("{other:?}"),
        };
        let rotate = |id| {
            inherited(&doc, id, b"Rotate")
                .and_then(|o| o.as_i64().ok())
                .unwrap_or(0)
        };
        assert_eq!((height(pages[0]), rotate(pages[0])), (792, 0));
        assert_eq!((height(pages[1]), rotate(pages[1])), (1008, 90));
        // Both pages still render their own text (their own font resources).
        let p = pdfium(&assets()).unwrap();
        let pdf = p.load_pdf_from_file(&out, None).unwrap();
        let texts: Vec<String> = pdf
            .pages()
            .iter()
            .map(|pg| pg.text().unwrap().all().trim().to_string())
            .collect();
        assert_eq!(texts, ["LETTER", "LEGAL ROTATED"]);
    }

    #[test]
    fn merged_forms_keep_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.pdf"), dir.path().join("b.pdf"));
        write_inheriting_pdf(&a, "A", 792, 0, Some(("Name", "Smith")));
        write_inheriting_pdf(&b, "B", 792, 0, Some(("Name", "Jones")));
        let out = dir.path().join("out.pdf");
        merge_pdfs(&[(a, "a".into()), (b, "b".into())], &out).unwrap();
        let doc = lopdf::Document::load(&out).unwrap();
        let form = doc.catalog().unwrap().get(b"AcroForm").unwrap();
        let (_, form) = doc.dereference(form).unwrap();
        let fields = form
            .as_dict()
            .unwrap()
            .get(b"Fields")
            .unwrap()
            .as_array()
            .unwrap();
        let named: Vec<(String, String)> = fields
            .iter()
            .map(|f| {
                let d = doc.get_dictionary(f.as_reference().unwrap()).unwrap();
                let s = |k: &[u8]| {
                    String::from_utf8(d.get(k).unwrap().as_str().unwrap().to_vec()).unwrap()
                };
                (s(b"T"), s(b"V"))
            })
            .collect();
        assert_eq!(
            named,
            [
                ("Name".into(), "Smith".into()),
                ("doc2_Name".into(), "Jones".into())
            ]
        );
    }

    #[test]
    fn page_texts_render_and_split() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("all.pdf");
        let texts: Vec<String> = (1..=5).map(|n| format!("page {n}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        write_test_pdf(&src, &refs);
        let p = pdfium(&assets()).unwrap();
        let got: Vec<String> = page_texts(p, &src)
            .unwrap()
            .iter()
            .map(|t| t.trim().to_string())
            .collect();
        assert_eq!(got, texts);

        let img = render_page(p, &src, 2, 120).unwrap();
        assert_eq!(img.size[0], 120);
        assert!(img.size[1] > 120, "portrait page");

        let outs = [dir.path().join("a.pdf"), dir.path().join("b.pdf")];
        split_pdf(&src, &[0..2, 2..5], &outs).unwrap();
        let read = |path: &Path| -> Vec<String> {
            page_texts(p, path)
                .unwrap()
                .iter()
                .map(|t| t.trim().to_string())
                .collect()
        };
        assert_eq!(read(&outs[0]), ["page 1", "page 2"]);
        assert_eq!(read(&outs[1]), ["page 3", "page 4", "page 5"]);
        assert!(
            split_pdf(&src, std::slice::from_ref(&(0..1)), &[outs[0].clone()]).is_err(),
            "never overwrites"
        );
        assert!(
            split_pdf(
                &src,
                std::slice::from_ref(&(4..9)),
                &[dir.path().join("c.pdf")]
            )
            .is_err()
        );
    }

    /// Two pages, one fillable field on each (names `Page1`, `Page2`).
    fn write_two_page_form(path: &Path) {
        use lopdf::{Document, Object, dictionary};
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut kids = Vec::new();
        let mut fields = Vec::new();
        for n in 1..=2 {
            let page_id = doc.new_object_id();
            let widget = doc.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx",
                "T" => Object::string_literal(format!("Page{n}")),
                "V" => Object::string_literal(format!("secret {n}")),
                "Rect" => vec![72.into(), 600.into(), 300.into(), 620.into()], "P" => page_id,
            });
            doc.objects.insert(
                page_id,
                Object::Dictionary(dictionary! {
                    "Type" => "Page", "Parent" => pages_id, "Annots" => vec![widget.into()],
                    "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                }),
            );
            kids.push(page_id.into());
            fields.push(widget.into());
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 2 }),
        );
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog", "Pages" => pages_id,
            "AcroForm" => dictionary! { "Fields" => fields },
        });
        doc.trailer.set("Root", catalog);
        doc.save(path).unwrap();
    }

    #[test]
    fn split_pieces_keep_only_their_own_form_fields() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("form.pdf");
        write_two_page_form(&src);
        let outs = [dir.path().join("p1.pdf"), dir.path().join("p2.pdf")];
        split_pdf(&src, &[0..1, 1..2], &outs).unwrap();
        for (out, mine, other) in [
            (&outs[0], "Page1", "secret 2"),
            (&outs[1], "Page2", "secret 1"),
        ] {
            let doc = lopdf::Document::load(out).unwrap();
            let form = doc.catalog().unwrap().get(b"AcroForm").unwrap();
            let (_, form) = doc.dereference(form).unwrap();
            let fields = form
                .as_dict()
                .unwrap()
                .get(b"Fields")
                .unwrap()
                .as_array()
                .unwrap();
            assert_eq!(fields.len(), 1);
            let d = doc
                .get_dictionary(fields[0].as_reference().unwrap())
                .unwrap();
            assert_eq!(d.get(b"T").unwrap().as_str().unwrap(), mine.as_bytes());
            // The other page's value is not anywhere in the piece.
            let bytes = std::fs::read(out).unwrap();
            let raw = String::from_utf8_lossy(&bytes);
            assert!(!raw.contains(other), "{} leaks {other}", out.display());
        }
    }

    #[test]
    fn failed_split_leaves_no_partial_pieces() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("all.pdf");
        write_test_pdf(&src, &["a", "b", "c"]);
        let first = dir.path().join("one.pdf");
        let second = dir.path().join("missing-folder").join("two.pdf");
        assert!(split_pdf(&src, &[0..1, 1..3], &[first.clone(), second]).is_err());
        assert!(
            !first.exists(),
            "the piece written before the failure is removed"
        );
    }

    #[test]
    fn sniffer_reads_in_background_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("o.pdf");
        write_test_pdf(&path, &["2025 Tax Organizer"]);
        let ctx = egui::Context::default();
        let mut s = Sniffer::new(&ctx, Some(assets()));
        assert!(s.enabled());
        s.request(&path);
        assert_eq!(s.get(&path), Some(&PageText::Pending));
        for _ in 0..200 {
            s.poll();
            if !matches!(s.get(&path), Some(PageText::Pending)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        match s.get(&path) {
            Some(PageText::Text(t)) => assert!(t.contains("Organizer")),
            other => panic!("{other:?}"),
        }
        let off = Sniffer::new(&ctx, None);
        assert!(!off.enabled());
    }
}
