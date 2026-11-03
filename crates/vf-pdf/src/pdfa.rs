//! PDF/A-2b (P04 step 4, QUESTIONS Q-28).
//!
//! [`convert`]: annotations and form fields are flattened, JavaScript and embedded files go,
//! every page drawn with a font that is not embedded is rebuilt as a 300 dpi image with an
//! invisible text layer (PDF/A-2 does not require fonts used only for invisible text to be
//! embedded), then an sRGB output intent, PDF/A XMP metadata and a file ID are added.
//!
//! [`check`]: the conformance points this tool can break or fix, checked on any file (veraPDF
//! needs Java and is not on this PC; the done-when's veraPDF run is an optional step elsewhere).

use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};
use pdfium_render::prelude::Pdfium;

use crate::layer::{self, PageWords};
use crate::words::OcrPage;

/// The font resource the invisible text layer uses ([`crate::layer`]).
const LAYER_FONT: &[u8] = b"VfOcrCourier";

/// What [`convert`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Pages rebuilt as images (fonts not embedded), 0-based.
    pub rasterized: Vec<usize>,
    pub flattened: usize,
}

/// Writes a PDF/A-2b copy of `src` to `out`. `now` is RFC 3339.
pub fn convert(
    pdfium: &Pdfium,
    src: &Path,
    out: &Path,
    title: &str,
    now: &str,
) -> Result<Report, String> {
    let bytes = std::fs::read(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    let mut doc = Document::load_mem(&bytes).map_err(|e| format!("cannot read the PDF: {e}"))?;
    if doc.is_encrypted() {
        return Err("the PDF is password protected".into());
    }
    let flattened = crate::flatten::flatten_doc(&mut doc)?;
    strip_forbidden(&mut doc);

    let page_ids: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    let rasterize: Vec<usize> = page_ids
        .iter()
        .enumerate()
        .filter(|(_, id)| !fonts_embedded(&doc, **id))
        .map(|(i, _)| i)
        .collect();
    let mut layers: Vec<(u32, OcrPage)> = Vec::new();
    if !rasterize.is_empty() {
        // Render from the flattened document, so burnt-in annotations show in the image.
        let mut flat = Vec::new();
        doc.save_to(&mut flat)
            .map_err(|e| format!("cannot write the PDF: {e}"))?;
        let pdf = pdfium
            .load_pdf_from_byte_vec(flat, None)
            .map_err(|e| format!("cannot open the PDF: {e}"))?;
        let geoms = layer::geometries(&doc);
        for &p in &rasterize {
            layers.push(crate::raster::rebuild_page(
                &pdf,
                &mut doc,
                &geoms[p],
                page_ids[p],
                p,
                &[],
            )?);
        }
    }
    doc.prune_objects();
    doc.version = "1.7".into();
    add_output_intent(&mut doc)?;
    add_metadata(&mut doc, title, now)?;
    let id = uuid_bytes(&bytes);
    doc.trailer.set(
        "ID",
        Object::Array(vec![
            Object::String(id.clone(), StringFormat::Hexadecimal),
            Object::String(id, StringFormat::Hexadecimal),
        ]),
    );
    let tmp = out.with_extension("pdfa.tmp");
    let mut buf = Vec::new();
    doc.save_to(&mut buf)
        .map_err(|e| format!("cannot write the PDF: {e}"))?;
    std::fs::write(&tmp, buf).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    let words: Vec<PageWords> = layers
        .iter()
        .map(|(page, ocr)| PageWords { page: *page, ocr })
        .collect();
    let r = if words.is_empty() {
        std::fs::rename(&tmp, out).map_err(|e| format!("cannot write {}: {e}", out.display()))
    } else {
        let r = layer::add_text_layer(&tmp, out, &words);
        let _ = std::fs::remove_file(&tmp);
        r
    };
    r?;
    let issues = check(out)?;
    if !issues.is_empty() {
        return Err(format!(
            "the PDF/A copy does not pass: {}",
            issues.join("; ")
        ));
    }
    Ok(Report {
        rasterized: rasterize,
        flattened,
    })
}

/// JavaScript, embedded files, actions on open, the info dictionary (PDF/A-2 would require it
/// to match the XMP), interpolation flags on images.
fn strip_forbidden(doc: &mut Document) {
    doc.trailer.remove(b"Info");
    let names_ref = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Names").ok())
        .and_then(|o| o.as_reference().ok());
    if let Ok(cat) = doc.catalog_mut() {
        cat.remove(b"OpenAction");
        cat.remove(b"AA");
        if let Ok(Object::Dictionary(n)) = cat.get_mut(b"Names") {
            n.remove(b"JavaScript");
            n.remove(b"EmbeddedFiles");
        }
    }
    if let Some(id) = names_ref
        && let Ok(n) = doc.get_dictionary_mut(id)
    {
        n.remove(b"JavaScript");
        n.remove(b"EmbeddedFiles");
    }
    for obj in doc.objects.values_mut() {
        let dict = match obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &mut s.dict,
            _ => continue,
        };
        dict.remove(b"AA");
        if dict
            .get(b"Interpolate")
            .and_then(Object::as_bool)
            .unwrap_or(false)
        {
            dict.set("Interpolate", false);
        }
    }
}

/// Whether every font page `page` draws with (its resources and the forms they use) is
/// embedded; the text layer's own font, used only for invisible text, does not count.
fn fonts_embedded(doc: &Document, page: ObjectId) -> bool {
    // A page's own /Resources replace inherited ones; without them, the nearest ancestor's.
    let res = match crate::layer::inherited_resources(doc, page) {
        Object::Dictionary(d) => d,
        _ => return true,
    };
    resources_ok(doc, &res, 0)
}

fn resources_ok(doc: &Document, res: &Dictionary, depth: usize) -> bool {
    if depth > 8 {
        return true;
    }
    let deref = |o: &Object| doc.dereference(o).ok().map(|(_, o)| o.clone());
    if let Some(Object::Dictionary(fonts)) = res.get(b"Font").ok().and_then(deref) {
        for (name, f) in fonts.iter() {
            if name.as_slice() == LAYER_FONT {
                continue;
            }
            let Some(Object::Dictionary(font)) = deref(f) else {
                continue;
            };
            if !font_embedded(doc, &font) {
                return false;
            }
        }
    }
    if let Some(Object::Dictionary(xobjects)) = res.get(b"XObject").ok().and_then(deref) {
        for (_, x) in xobjects.iter() {
            if let Some(Object::Stream(s)) = deref(x)
                && s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Form")
                && let Some(Object::Dictionary(r)) = s.dict.get(b"Resources").ok().and_then(deref)
                && !resources_ok(doc, &r, depth + 1)
            {
                return false;
            }
        }
    }
    true
}

fn font_embedded(doc: &Document, font: &Dictionary) -> bool {
    let deref = |o: &Object| doc.dereference(o).ok().map(|(_, o)| o.clone());
    let subtype = font
        .get(b"Subtype")
        .and_then(Object::as_name)
        .unwrap_or_default();
    if subtype == b"Type3" {
        return true; // glyphs are content streams
    }
    let target = if subtype == b"Type0" {
        match font.get(b"DescendantFonts").ok().and_then(deref) {
            Some(Object::Array(a)) => match a.first().and_then(deref) {
                Some(Object::Dictionary(d)) => d,
                _ => return false,
            },
            _ => return false,
        }
    } else {
        font.clone()
    };
    match target.get(b"FontDescriptor").ok().and_then(deref) {
        Some(Object::Dictionary(fd)) => {
            fd.has(b"FontFile") || fd.has(b"FontFile2") || fd.has(b"FontFile3")
        }
        _ => false,
    }
}

fn add_output_intent(doc: &mut Document) -> Result<(), String> {
    let icc = doc.add_object(Stream::new(
        Dictionary::from_iter(vec![("N", 3.into())]),
        srgb_icc(),
    ));
    let intent = doc.add_object(Dictionary::from_iter(vec![
        ("Type", Object::Name(b"OutputIntent".to_vec())),
        ("S", Object::Name(b"GTS_PDFA1".to_vec())),
        (
            "OutputConditionIdentifier",
            Object::string_literal("sRGB IEC61966-2.1"),
        ),
        ("Info", Object::string_literal("sRGB")),
        ("DestOutputProfile", Object::Reference(icc)),
    ]));
    doc.catalog_mut()
        .map_err(|e| e.to_string())?
        .set("OutputIntents", vec![Object::Reference(intent)]);
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn add_metadata(doc: &mut Document, title: &str, now: &str) -> Result<(), String> {
    let t = xml_escape(title);
    let date = now.get(..19).map(|d| format!("{d}Z")).unwrap_or_default();
    let xmp = format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">
  <rdf:Description rdf:about=\"\"
    xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"
    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"
    xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">
   <pdfaid:part>2</pdfaid:part>
   <pdfaid:conformance>B</pdfaid:conformance>
   <dc:format>application/pdf</dc:format>
   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{t}</rdf:li></rdf:Alt></dc:title>
   <xmp:CreateDate>{date}</xmp:CreateDate>
   <xmp:ModifyDate>{date}</xmp:ModifyDate>
   <xmp:MetadataDate>{date}</xmp:MetadataDate>
   <xmp:CreatorTool>Vibe Files</xmp:CreatorTool>
   <pdf:Producer>Vibe Files</pdf:Producer>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end=\"w\"?>"
    );
    let id = doc.add_object(Stream::new(
        Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Metadata".to_vec())),
            ("Subtype", Object::Name(b"XML".to_vec())),
        ]),
        xmp.into_bytes(),
    ));
    doc.catalog_mut()
        .map_err(|e| e.to_string())?
        .set("Metadata", Object::Reference(id));
    Ok(())
}

/// A file ID from the source bytes (stable for the same input).
fn uuid_bytes(src: &[u8]) -> Vec<u8> {
    use std::hash::{Hash, Hasher};
    let mut a = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut a);
    let mut b = std::collections::hash_map::DefaultHasher::new();
    (src.len(), a.finish()).hash(&mut b);
    let mut v = a.finish().to_be_bytes().to_vec();
    v.extend(b.finish().to_be_bytes());
    v
}

/// A compact ICC v2 display profile for sRGB (D50-adapted primaries, gamma 2.2 curves): the
/// output intent's `DestOutputProfile`. Built here so no profile file ships.
pub fn srgb_icc() -> Vec<u8> {
    fn s15(v: f64) -> [u8; 4] {
        ((v * 65536.0).round() as i32).to_be_bytes()
    }
    fn xyz(x: f64, y: f64, z: f64) -> Vec<u8> {
        let mut t = b"XYZ \0\0\0\0".to_vec();
        for v in [x, y, z] {
            t.extend(s15(v));
        }
        t
    }
    fn desc(text: &str) -> Vec<u8> {
        let mut t = b"desc\0\0\0\0".to_vec();
        t.extend(((text.len() + 1) as u32).to_be_bytes());
        t.extend(text.as_bytes());
        t.push(0);
        t.extend([0u8; 4]); // unicode language
        t.extend([0u8; 4]); // unicode count
        t.extend([0u8; 2]); // scriptcode code
        t.push(0); // scriptcode count
        t.extend([0u8; 67]);
        t
    }
    let text = |s: &str| {
        let mut t = b"text\0\0\0\0".to_vec();
        t.extend(s.as_bytes());
        t.push(0);
        t
    };
    let curv = {
        let mut t = b"curv\0\0\0\0".to_vec();
        t.extend(1u32.to_be_bytes());
        t.extend(0x0233u16.to_be_bytes()); // gamma 2.2 (u8Fixed8)
        t.extend([0u8; 2]);
        t
    };
    // (signature, data); the three TRC tags share one curve.
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc("sRGB IEC61966-2.1 (Vibe Files)")),
        (b"cprt", text("No copyright, use freely")),
        (b"wtpt", xyz(0.9642, 1.0, 0.8249)),
        (b"rXYZ", xyz(0.4361, 0.2225, 0.0139)),
        (b"gXYZ", xyz(0.3851, 0.7169, 0.0971)),
        (b"bXYZ", xyz(0.1431, 0.0606, 0.7141)),
        (b"rTRC", curv.clone()),
        (b"gTRC", curv.clone()),
        (b"bTRC", curv),
    ];
    let table_len = 4 + 12 * tags.len();
    let mut offset = 128 + table_len;
    let mut table = (tags.len() as u32).to_be_bytes().to_vec();
    let mut data = Vec::new();
    let mut trc_at: Option<(u32, u32)> = None;
    for (sig, body) in &tags {
        let shared = trc_at.filter(|_| sig.ends_with(b"TRC"));
        let (off, len) = if let Some(at) = shared {
            at
        } else {
            let at = (offset as u32, body.len() as u32);
            data.extend(body);
            while data.len() % 4 != 0 {
                data.push(0);
            }
            offset = 128 + table_len + data.len();
            if sig.ends_with(b"TRC") {
                trc_at = Some(at);
            }
            at
        };
        table.extend(sig.iter());
        table.extend(off.to_be_bytes());
        table.extend(len.to_be_bytes());
    }
    let size = 128 + table.len() + data.len();
    let mut h = Vec::with_capacity(size);
    h.extend((size as u32).to_be_bytes());
    h.extend(b"none");
    h.extend(0x0210_0000u32.to_be_bytes()); // version 2.1
    h.extend(b"mntr");
    h.extend(b"RGB ");
    h.extend(b"XYZ ");
    for v in [2026u16, 1, 1, 0, 0, 0] {
        h.extend(v.to_be_bytes());
    }
    h.extend(b"acsp");
    h.extend([0u8; 4]); // platform
    h.extend([0u8; 4]); // flags
    h.extend([0u8; 4]); // manufacturer
    h.extend([0u8; 4]); // model
    h.extend([0u8; 8]); // attributes
    h.extend([0u8; 4]); // rendering intent: perceptual
    for v in [0.9642, 1.0, 0.8249] {
        h.extend(s15(v));
    }
    h.extend([0u8; 4]); // creator
    h.extend([0u8; 16]); // profile ID
    h.extend([0u8; 28]);
    debug_assert_eq!(h.len(), 128);
    h.extend(table);
    h.extend(data);
    h
}

/// What keeps `path` from being PDF/A-2b, among the points this tool handles. Empty = passes.
pub fn check(path: &Path) -> Result<Vec<String>, String> {
    let raw = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut issues = Vec::new();
    let second_line = raw
        .iter()
        .position(|&b| b == b'\n')
        .map(|i| &raw[i + 1..(i + 6).min(raw.len())]);
    if !raw.starts_with(b"%PDF-1.")
        || !second_line.is_some_and(|l| l[0] == b'%' && l[1..5].iter().all(|&b| b >= 128))
    {
        issues.push("header: no binary comment line".into());
    }
    let doc = Document::load_mem(&raw).map_err(|e| format!("cannot read the PDF: {e}"))?;
    if doc.is_encrypted() {
        issues.push("encrypted".into());
    }
    if doc.trailer.get(b"ID").is_err() {
        issues.push("no file ID in the trailer".into());
    }
    let deref = |o: &Object| doc.dereference(o).ok().map(|(_, o)| o.clone());
    let cat = doc.catalog().map_err(|e| e.to_string())?;
    match cat.get(b"Metadata").ok().and_then(deref) {
        Some(Object::Stream(s)) => {
            let x = String::from_utf8_lossy(&s.content);
            if !x.contains("<pdfaid:part>2</pdfaid:part>")
                || !x.contains("<pdfaid:conformance>B</pdfaid:conformance>")
            {
                issues.push("XMP does not say PDF/A-2b".into());
            }
            if s.dict.has(b"Filter") {
                issues.push("XMP metadata is compressed".into());
            }
        }
        _ => issues.push("no XMP metadata".into()),
    }
    let intent_ok = match cat.get(b"OutputIntents").ok().and_then(deref) {
        Some(Object::Array(a)) => a.iter().filter_map(deref).any(|i| match i {
            Object::Dictionary(d) => {
                d.get(b"S").and_then(Object::as_name).ok() == Some(b"GTS_PDFA1")
                    && matches!(
                        d.get(b"DestOutputProfile").ok().and_then(deref),
                        Some(Object::Stream(_))
                    )
            }
            _ => false,
        }),
        _ => false,
    };
    if !intent_ok {
        issues.push("no PDF/A output intent with an ICC profile".into());
    }
    if doc.trailer.get(b"Info").is_ok() {
        // Allowed when it matches the XMP; this tool never writes one, so flag it.
        issues.push("document info dictionary present".into());
    }
    for (id, obj) in &doc.objects {
        let dict = match obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        if dict.has(b"JS") || dict.get(b"S").and_then(Object::as_name).ok() == Some(b"JavaScript") {
            issues.push(format!("JavaScript in object {}", id.0));
        }
        if dict.has(b"EmbeddedFiles") {
            issues.push("embedded files".into());
        }
        let lzw = |f: &Object| match f {
            Object::Name(n) => n == b"LZWDecode",
            Object::Array(a) => a.iter().any(|x| x.as_name().ok() == Some(b"LZWDecode")),
            _ => false,
        };
        if dict.get(b"Filter").is_ok_and(lzw) {
            issues.push(format!("LZW compression in object {}", id.0));
        }
        if dict
            .get(b"Interpolate")
            .and_then(Object::as_bool)
            .unwrap_or(false)
        {
            issues.push(format!("image interpolation in object {}", id.0));
        }
    }
    for (i, (_, page)) in doc.get_pages().into_iter().enumerate() {
        if !fonts_embedded(&doc, page) {
            issues.push(format!("page {}: a font is not embedded", i + 1));
        }
        let annots = doc
            .get_dictionary(page)
            .ok()
            .and_then(|d| d.get(b"Annots").ok().cloned())
            .and_then(|o| deref(&o));
        if let Some(Object::Array(a)) = annots {
            for x in a.iter().filter_map(deref) {
                let Object::Dictionary(d) = x else { continue };
                let sub = d
                    .get(b"Subtype")
                    .and_then(Object::as_name)
                    .unwrap_or_default();
                let f = d.get(b"F").and_then(Object::as_i64).unwrap_or(0);
                if sub != b"Popup" && f & 4 == 0 {
                    issues.push(format!("page {}: an annotation does not print", i + 1));
                }
                if sub != b"Popup" && sub != b"Link" && !d.has(b"AP") {
                    issues.push(format!("page {}: an annotation has no appearance", i + 1));
                }
            }
        }
    }
    issues.sort();
    issues.dedup();
    Ok(issues)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icc_profile_is_well_formed() {
        let p = srgb_icc();
        assert_eq!(
            u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize,
            p.len()
        );
        assert_eq!(&p[36..40], b"acsp");
        assert_eq!(&p[12..20], b"mntrRGB ");
        let n = u32::from_be_bytes(p[128..132].try_into().unwrap()) as usize;
        assert_eq!(n, 9);
        for i in 0..n {
            let e = &p[132 + 12 * i..144 + 12 * i];
            let (off, len) = (
                u32::from_be_bytes(e[4..8].try_into().unwrap()) as usize,
                u32::from_be_bytes(e[8..12].try_into().unwrap()) as usize,
            );
            assert!(off % 4 == 0 && off + len <= p.len(), "tag {i}");
        }
    }
}
