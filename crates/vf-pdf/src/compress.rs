//! Compress scanned images (P04 step 7): every 8-bit gray or RGB image drawn larger than
//! `max_dpi` on its page is downsampled, and re-encoded as JPEG when that is smaller. Images
//! with masks, decode arrays, other colour spaces or bit depths are left alone, as are 1-bit
//! scans (JBIG2 / CCITT would need an encoder this build does not have).

use std::collections::HashMap;
use std::path::Path;

use image::{DynamicImage, imageops};
use lopdf::{Document, Object, ObjectId};

/// What [`compress`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub images: usize,
    pub before: u64,
    pub after: u64,
}

/// Writes a compressed copy of `src` to `out`.
pub fn compress(src: &Path, out: &Path, max_dpi: f32, quality: u8) -> Result<Report, String> {
    let before = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
    let mut doc = Document::load(src).map_err(|e| format!("cannot read the PDF: {e}"))?;
    if doc.is_encrypted() {
        return Err("the PDF is password protected".into());
    }
    // Largest pixel size each image may keep: its page's size at `max_dpi`.
    let mut limit: HashMap<ObjectId, (u32, u32)> = HashMap::new();
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    for page in pages {
        let g = crate::layer::Geometry::of(&doc, page)?;
        let (w, h) = g.display();
        let (lw, lh) = (
            (w / 72.0 * max_dpi).ceil() as u32,
            (h / 72.0 * max_dpi).ceil() as u32,
        );
        let (lw, lh) = (lw.max(lh), lw.max(lh)); // the image may be turned on the page
        if let Object::Dictionary(res) = crate::layer::inherited_resources(&doc, page)
            && let Ok(xo) = res.get(b"XObject")
            && let Ok((_, Object::Dictionary(xo))) = doc.dereference(xo)
        {
            for (_, x) in xo.iter() {
                if let Ok(id) = x.as_reference() {
                    let e = limit.entry(id).or_insert((0, 0));
                    *e = (e.0.max(lw), e.1.max(lh));
                }
            }
        }
    }
    let mut images = 0;
    for (id, (lw, lh)) in limit {
        if let Some(new) = recompress(&doc, id, lw, lh, quality)?
            && let Ok(Object::Stream(s)) = doc.get_object_mut(id)
        {
            let (bytes, w, h) = new;
            s.dict.set("Width", w as i64);
            s.dict.set("Height", h as i64);
            s.dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
            s.dict.remove(b"DecodeParms");
            s.set_content(bytes);
            images += 1;
        }
    }
    doc.prune_objects();
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)
        .map_err(|e| format!("cannot write the PDF: {e}"))?;
    std::fs::write(out, &bytes).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(Report {
        images,
        before,
        after: bytes.len() as u64,
    })
}

/// The image `id` as a smaller JPEG (bytes, width, height), or `None` to leave it.
fn recompress(
    doc: &Document,
    id: ObjectId,
    max_w: u32,
    max_h: u32,
    quality: u8,
) -> Result<Option<(Vec<u8>, u32, u32)>, String> {
    let Ok(Object::Stream(s)) = doc.get_object(id) else {
        return Ok(None);
    };
    let d = &s.dict;
    if d.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Image")
        || d.has(b"SMask")
        || d.has(b"Mask")
        || d.has(b"Decode")
        || d.get(b"ImageMask")
            .and_then(Object::as_bool)
            .unwrap_or(false)
        || d.get(b"BitsPerComponent").and_then(Object::as_i64).ok() != Some(8)
    {
        return Ok(None);
    }
    let comps = match d.get(b"ColorSpace").and_then(Object::as_name) {
        Ok(b"DeviceGray") => 1,
        Ok(b"DeviceRGB") => 3,
        _ => return Ok(None),
    };
    let (w, h) = (
        d.get(b"Width").and_then(Object::as_i64).unwrap_or(0) as u32,
        d.get(b"Height").and_then(Object::as_i64).unwrap_or(0) as u32,
    );
    if w == 0 || h == 0 {
        return Ok(None);
    }
    let filter = match d.get(b"Filter") {
        Ok(Object::Name(n)) => Some(n.clone()),
        Ok(Object::Array(a)) if a.len() == 1 => a[0].as_name().ok().map(<[u8]>::to_vec),
        Ok(_) => return Ok(None),
        Err(_) => None,
    };
    let img: DynamicImage = match filter.as_deref() {
        Some(b"DCTDecode") => {
            match image::load_from_memory_with_format(&s.content, image::ImageFormat::Jpeg) {
                Ok(i) => i,
                Err(_) => return Ok(None),
            }
        }
        None | Some(b"FlateDecode") => {
            let raw = if filter.is_some() {
                match s.decompressed_content() {
                    Ok(r) => r,
                    Err(_) => return Ok(None),
                }
            } else {
                s.content.clone()
            };
            if raw.len() < (w * h) as usize * comps {
                return Ok(None);
            }
            let raw = raw[..(w * h) as usize * comps].to_vec();
            if comps == 1 {
                DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, raw).ok_or("bad image")?)
            } else {
                DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, raw).ok_or("bad image")?)
            }
        }
        _ => return Ok(None),
    };
    let scale = (max_w as f32 / w as f32)
        .min(max_h as f32 / h as f32)
        .min(1.0);
    let img = if scale < 0.95 {
        img.resize(
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
            imageops::FilterType::CatmullRom,
        )
    } else {
        img
    };
    let mut jpeg = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality);
    let r = if comps == 1 {
        img.to_luma8().write_with_encoder(enc)
    } else {
        img.to_rgb8().write_with_encoder(enc)
    };
    r.map_err(|e| format!("cannot encode an image: {e}"))?;
    if jpeg.len() >= s.content.len() && scale >= 0.95 {
        return Ok(None); // not smaller
    }
    Ok(Some((jpeg, img.width(), img.height())))
}
