//! Images filed as PDFs (V25, owner 2026-11-09): a JPEG, PNG, TIFF (every page), BMP or GIF
//! becomes a PDF with one page per image. Each page is the image fitted inside 8.5 × 11 inches
//! (turned to landscape for a wide image), with no margin.
//!
//! A JPEG that needs no turning keeps its own bytes (no quality lost); a phone photo that is
//! stored sideways is turned upright by its EXIF orientation and saved again as a JPEG. Other
//! images are stored lossless (Flate). HEIC is not read: such files are filed as they are.

use std::path::Path;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use lopdf::{Document, Object, Stream, dictionary};

/// File extensions turned into a PDF when filed (lower case).
pub const CONVERTIBLE: [&str; 7] = ["jpg", "jpeg", "png", "tif", "tiff", "bmp", "gif"];

/// Whether `path` is an image this module can turn into a PDF.
pub fn convertible(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| CONVERTIBLE.contains(&e.to_ascii_lowercase().as_str()))
}

/// Largest page side and the other side, in points (US Letter).
const LONG_SIDE: f32 = 792.0;
const SHORT_SIDE: f32 = 612.0;

/// One page's picture, ready to go into the PDF: an image object, already compressed (pages
/// are compressed as they are read, so a long TIFF never holds all its pages raw).
struct Picture {
    width: u32,
    height: u32,
    stream: Stream,
}

impl Picture {
    /// JPEG bytes stored as they are (`DCTDecode`).
    fn jpeg(width: u32, height: u32, gray: bool, data: Vec<u8>) -> Self {
        let mut stream = Stream::new(image_dict(width, height, gray, 8), data);
        stream.dict.set("Filter", "DCTDecode");
        stream.allows_compression = false;
        Self {
            width,
            height,
            stream,
        }
    }

    /// Raw samples (`bits` 1 or 8 per component, rows padded to whole bytes), compressed now.
    fn raw(width: u32, height: u32, gray: bool, bits: i64, data: Vec<u8>) -> Result<Self, String> {
        let mut stream = Stream::new(image_dict(width, height, gray, bits), data);
        stream
            .compress()
            .map_err(|e| format!("cannot compress a page: {e}"))?;
        Ok(Self {
            width,
            height,
            stream,
        })
    }
}

fn image_dict(width: u32, height: u32, gray: bool, bits: i64) -> lopdf::Dictionary {
    dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => width as i64,
        "Height" => height as i64,
        "ColorSpace" => if gray { "DeviceGray" } else { "DeviceRGB" },
        "BitsPerComponent" => bits,
    }
}

/// Writes `out` (a new file) from the image `src`; returns the page count. A half-written `out`
/// is removed.
pub fn image_to_pdf(src: &Path, out: &Path) -> Result<usize, String> {
    if out.exists() {
        return Err(format!("{} already exists", out.display()));
    }
    let pictures = read_pictures(src)?;
    if pictures.is_empty() {
        return Err(format!("{} has no picture in it", src.display()));
    }
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids: Vec<Object> = Vec::new();
    for (i, p) in pictures.into_iter().enumerate() {
        let (w, h) = page_size(p.width, p.height);
        let image_id = doc.add_object(p.stream);
        let name = format!("Im{}", i + 1);
        let content = format!("q {w:.2} 0 0 {h:.2} 0 0 cm /{name} Do Q");
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)],
            "Resources" => dictionary! {
                "XObject" => dictionary! { name.as_str() => image_id },
            },
        });
        kids.push(page_id.into());
    }
    let count = kids.len();
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => kids, "Count" => count as i64,
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    if let Err(e) = crate::pdf::save_new(&mut doc, out) {
        // Ours: it did not exist above.
        let _ = std::fs::remove_file(out);
        return Err(e);
    }
    Ok(count)
}

/// The page that fits a `w` × `h` pixel picture inside Letter (portrait or landscape).
fn page_size(w: u32, h: u32) -> (f32, f32) {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let (max_w, max_h) = if w > h {
        (LONG_SIDE, SHORT_SIDE)
    } else {
        (SHORT_SIDE, LONG_SIDE)
    };
    let scale = (max_w / w).min(max_h / h);
    let round = |x: f32| (x * 100.0).round() / 100.0;
    (round(w * scale), round(h * scale))
}

fn read_pictures(src: &Path) -> Result<Vec<Picture>, String> {
    let reader = ImageReader::open(src)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| format!("cannot open {}: {e}", src.display()))?;
    match reader.format() {
        Some(ImageFormat::Tiff) => read_tiff(src),
        Some(ImageFormat::Jpeg) => read_jpeg(src),
        Some(_) => {
            let img = decode_upright(src)?;
            Ok(vec![lossless(img)?])
        }
        None => Err(format!(
            "{} is not a picture this program reads",
            src.display()
        )),
    }
}

/// The image turned upright by its EXIF orientation.
fn decode_upright(src: &Path) -> Result<DynamicImage, String> {
    let bad = |e: image::ImageError| format!("cannot read {}: {e}", src.display());
    let mut decoder = ImageReader::open(src)
        .map_err(|e| format!("cannot open {}: {e}", src.display()))?
        .with_guessed_format()
        .map_err(|e| format!("cannot open {}: {e}", src.display()))?
        .into_decoder()
        .map_err(bad)?;
    let orientation = decoder.orientation().map_err(bad)?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(bad)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// A JPEG keeps its own bytes when it is upright and gray or RGB; otherwise it is turned and
/// saved again.
fn read_jpeg(src: &Path) -> Result<Vec<Picture>, String> {
    let bad = |e: image::ImageError| format!("cannot read {}: {e}", src.display());
    let mut reader =
        ImageReader::open(src).map_err(|e| format!("cannot open {}: {e}", src.display()))?;
    reader.set_format(ImageFormat::Jpeg);
    let mut decoder = reader.into_decoder().map_err(bad)?;
    let orientation = decoder.orientation().map_err(bad)?;
    let (width, height) = decoder.dimensions();
    let color = decoder.original_color_type();
    let upright = orientation == image::metadata::Orientation::NoTransforms;
    let plain = matches!(
        color,
        image::ExtendedColorType::L8 | image::ExtendedColorType::Rgb8
    );
    if upright && plain {
        let data = std::fs::read(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
        let gray = color == image::ExtendedColorType::L8;
        return Ok(vec![Picture::jpeg(width, height, gray, data)]);
    }
    let mut img = DynamicImage::from_decoder(decoder).map_err(bad)?;
    img.apply_orientation(orientation);
    let gray = !img.color().has_color();
    let mut data = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, 90);
    if gray {
        img.to_luma8().write_with_encoder(encoder).map_err(bad)?;
    } else {
        img.to_rgb8().write_with_encoder(encoder).map_err(bad)?;
    }
    Ok(vec![Picture::jpeg(img.width(), img.height(), gray, data)])
}

/// Raw 8-bit gray or RGB samples (transparent parts on white), compressed.
fn lossless(img: DynamicImage) -> Result<Picture, String> {
    let (width, height) = (img.width(), img.height());
    let alpha = img.color().has_alpha();
    if !img.color().has_color() {
        let gray = if alpha {
            flatten_alpha(img).to_luma8()
        } else {
            img.into_luma8()
        };
        return Picture::raw(width, height, true, 8, gray.into_raw());
    }
    let rgb = if alpha {
        flatten_alpha(img).to_rgb8()
    } else {
        img.into_rgb8()
    };
    Picture::raw(width, height, false, 8, rgb.into_raw())
}

/// Transparent parts become white (a PDF page is white under the picture).
fn flatten_alpha(img: DynamicImage) -> DynamicImage {
    let mut rgba = img.into_rgba8();
    for p in rgba.pixels_mut() {
        let a = p[3] as u32;
        for c in 0..3 {
            p[c] = ((p[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
        p[3] = 255;
    }
    DynamicImage::ImageRgba8(rgba)
}

/// Every page of a TIFF (scanners write multi-page TIFFs).
fn read_tiff(src: &Path) -> Result<Vec<Picture>, String> {
    use tiff::ColorType;
    use tiff::decoder::{Decoder, DecodingResult};
    let bad = |e: tiff::TiffError| format!("cannot read {}: {e}", src.display());
    let file =
        std::fs::File::open(src).map_err(|e| format!("cannot open {}: {e}", src.display()))?;
    let mut d = Decoder::new(std::io::BufReader::new(file)).map_err(bad)?;
    let mut out = Vec::new();
    loop {
        let (width, height) = d.dimensions().map_err(bad)?;
        let color = d.colortype().map_err(bad)?;
        let supported = matches!(
            color,
            ColorType::Gray(1 | 8 | 16)
                | ColorType::RGB(8 | 16)
                | ColorType::RGBA(8 | 16)
                | ColorType::GrayA(8 | 16)
        );
        if !supported {
            // Palette, CMYK and the rest: the image crate reads a one-page file; a longer one
            // is filed as it is rather than as a PDF missing pages.
            if out.is_empty() && !d.more_images() {
                tracing::info!(file = %src.display(), ?color, "TIFF read by the image crate");
                return Ok(vec![lossless(decode_upright(src)?)?]);
            }
            return Err(format!(
                "{}: TIFF pages in colors this program does not convert ({color:?})",
                src.display()
            ));
        }
        let samples = match d.read_image().map_err(bad)? {
            DecodingResult::U8(v) => v,
            DecodingResult::U16(v) => v.into_iter().map(|x| (x >> 8) as u8).collect(),
            _ => return Err(format!("{}: unsupported TIFF sample size", src.display())),
        };
        let damaged = || format!("{}: damaged TIFF page", src.display());
        let picture = match color {
            // One bit per pixel, rows padded to whole bytes, 1 = white (the decoder already
            // turned WhiteIsZero round): PDF's 1-bit gray as it is.
            ColorType::Gray(1) => Picture::raw(width, height, true, 1, samples)?,
            ColorType::Gray(_) => Picture::raw(width, height, true, 8, samples)?,
            ColorType::RGB(_) => Picture::raw(width, height, false, 8, samples)?,
            ColorType::RGBA(_) => {
                let img = image::RgbaImage::from_raw(width, height, samples).ok_or_else(damaged)?;
                lossless(DynamicImage::ImageRgba8(img))?
            }
            _ => {
                let img =
                    image::GrayAlphaImage::from_raw(width, height, samples).ok_or_else(damaged)?;
                lossless(DynamicImage::ImageLumaA8(img))?
            }
        };
        out.push(picture);
        if !d.more_images() {
            break;
        }
        d.next_image().map_err(bad)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages_and_boxes(pdf: &Path) -> Vec<(f32, f32)> {
        let doc = Document::load(pdf).unwrap();
        doc.get_pages()
            .values()
            .map(|id| {
                let page = doc.get_dictionary(*id).unwrap();
                let mb = page.get(b"MediaBox").unwrap().as_array().unwrap();
                (mb[2].as_float().unwrap(), mb[3].as_float().unwrap())
            })
            .collect()
    }

    #[test]
    fn png_jpeg_and_multi_page_tiff_become_pdfs() {
        let tmp = tempfile::tempdir().unwrap();
        // A wide RGB PNG with transparency → one landscape page.
        let png = tmp.path().join("receipt.png");
        image::RgbaImage::from_fn(400, 200, |x, _| image::Rgba([(x % 255) as u8, 10, 20, 128]))
            .save(&png)
            .unwrap();
        let out = tmp.path().join("receipt.pdf");
        assert_eq!(image_to_pdf(&png, &out).unwrap(), 1);
        assert_eq!(pages_and_boxes(&out), [(792.0, 396.0)]);
        // A tall gray JPEG keeps its own bytes → one portrait page.
        let jpg = tmp.path().join("photo.jpg");
        image::GrayImage::from_fn(300, 600, |x, y| image::Luma([((x + y) % 255) as u8]))
            .save(&jpg)
            .unwrap();
        let out = tmp.path().join("photo.pdf");
        assert_eq!(image_to_pdf(&jpg, &out).unwrap(), 1);
        assert_eq!(pages_and_boxes(&out), [(396.0, 792.0)]);
        let doc = Document::load(&out).unwrap();
        let jpeg_bytes = std::fs::read(&jpg).unwrap();
        assert!(
            doc.objects.values().any(|o| matches!(o, Object::Stream(s)
                if s.content == jpeg_bytes)),
            "the JPEG is stored as it was"
        );
        // A two-page TIFF → two pages.
        let tif = tmp.path().join("scan.tif");
        {
            let file = std::fs::File::create(&tif).unwrap();
            let mut enc = tiff::encoder::TiffEncoder::new(file).unwrap();
            for shade in [30u8, 200] {
                let data = vec![shade; 100 * 130];
                enc.write_image::<tiff::encoder::colortype::Gray8>(100, 130, &data)
                    .unwrap();
            }
        }
        let out = tmp.path().join("scan.pdf");
        assert_eq!(image_to_pdf(&tif, &out).unwrap(), 2);
        assert_eq!(crate::pdf::page_count(&out).unwrap(), 2);
        // Never over an existing file.
        assert!(image_to_pdf(&tif, &out).is_err());
        // Not a picture.
        let txt = tmp.path().join("notes.png");
        std::fs::write(&txt, b"not an image").unwrap();
        assert!(image_to_pdf(&txt, &tmp.path().join("notes.pdf")).is_err());
    }

    /// A black-and-white (1-bit, WhiteIsZero, as fax scanners write) TIFF keeps black as black.
    #[test]
    fn a_one_bit_tiff_keeps_its_blacks() {
        // 16 × 2 pixels, uncompressed, WhiteIsZero: row 0 = first 8 black, row 1 = last 8.
        let mut t: Vec<u8> = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        let entries: [(u16, u16, u32, u32); 8] = [
            (256, 3, 1, 16), // width
            (257, 3, 1, 2),  // height
            (258, 3, 1, 1),  // bits per sample
            (259, 3, 1, 1),  // no compression
            (262, 3, 1, 0),  // WhiteIsZero
            (273, 4, 1, 0),  // strip offset (set below)
            (278, 3, 1, 2),  // rows per strip
            (279, 4, 1, 4),  // strip bytes
        ];
        let data_at = 8 + 2 + entries.len() * 12 + 4;
        t.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in entries {
            let value = if tag == 273 { data_at as u32 } else { value };
            t.extend_from_slice(&tag.to_le_bytes());
            t.extend_from_slice(&kind.to_le_bytes());
            t.extend_from_slice(&count.to_le_bytes());
            t.extend_from_slice(&value.to_le_bytes());
        }
        t.extend_from_slice(&[0, 0, 0, 0]);
        t.extend_from_slice(&[0xFF, 0x00, 0x00, 0xFF]);
        let tmp = tempfile::tempdir().unwrap();
        let tif = tmp.path().join("fax.tif");
        std::fs::write(&tif, &t).unwrap();
        let out = tmp.path().join("fax.pdf");
        assert_eq!(image_to_pdf(&tif, &out).unwrap(), 1);
        let dir = crate::pdf::find_pdfium(Path::new("")).expect("pdfium in assets");
        let pdfium = crate::pdf::pdfium(&dir).unwrap();
        let img = crate::pdf::render_page(pdfium, &out, 0, 160).unwrap();
        let [w, h] = img.size;
        let px = |x: usize, y: usize| img.pixels[y * w + x].r();
        // Top-left black, top-right white; bottom-left white, bottom-right black.
        assert!(px(w / 8, h / 4) < 60, "top-left {}", px(w / 8, h / 4));
        assert!(
            px(w * 7 / 8, h / 4) > 200,
            "top-right {}",
            px(w * 7 / 8, h / 4)
        );
        assert!(
            px(w / 8, h * 3 / 4) > 200,
            "bottom-left {}",
            px(w / 8, h * 3 / 4)
        );
        assert!(
            px(w * 7 / 8, h * 3 / 4) < 60,
            "bottom-right {}",
            px(w * 7 / 8, h * 3 / 4)
        );
    }

    /// A multi-page TIFF in colors not converted page by page is refused (filed as it is), not
    /// turned into a PDF of its first page only (QA 2026-11-09).
    #[test]
    fn a_multi_page_cmyk_tiff_is_not_cut_to_one_page() {
        let tmp = tempfile::tempdir().unwrap();
        let tif = tmp.path().join("cmyk.tif");
        {
            let file = std::fs::File::create(&tif).unwrap();
            let mut enc = tiff::encoder::TiffEncoder::new(file).unwrap();
            for _ in 0..2 {
                let data = vec![10u8; 20 * 20 * 4];
                enc.write_image::<tiff::encoder::colortype::CMYK8>(20, 20, &data)
                    .unwrap();
            }
        }
        let out = tmp.path().join("cmyk.pdf");
        let e = image_to_pdf(&tif, &out).unwrap_err();
        assert!(e.contains("does not convert"), "{e}");
        assert!(!out.exists());
    }

    #[test]
    fn a_sideways_phone_photo_is_turned_upright() {
        // EXIF orientation 6 (turn 90° clockwise): stored 600 × 300, shown 300 × 600.
        let tmp = tempfile::tempdir().unwrap();
        let jpg = tmp.path().join("phone.jpg");
        let mut bytes = Vec::new();
        image::RgbImage::from_pixel(600, 300, image::Rgb([200, 100, 50]))
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
            .unwrap();
        // APP1 Exif segment with Orientation = 6, after the SOI marker.
        let tiff_hdr: Vec<u8> = [
            b"MM\0\x2a\0\0\0\x08".as_slice(),
            &[0, 1],                                     // one entry
            &[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0], // Orientation SHORT 6
            &[0, 0, 0, 0],                               // no next IFD
        ]
        .concat();
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff_hdr);
        let len = (app1.len() + 2) as u16;
        let mut with_exif = vec![0xFF, 0xD8, 0xFF, 0xE1];
        with_exif.extend_from_slice(&len.to_be_bytes());
        with_exif.extend_from_slice(&app1);
        with_exif.extend_from_slice(&bytes[2..]);
        std::fs::write(&jpg, &with_exif).unwrap();
        let out = tmp.path().join("phone.pdf");
        image_to_pdf(&jpg, &out).unwrap();
        assert_eq!(pages_and_boxes(&out), [(396.0, 792.0)]);
    }
}
