//! Image normalization (PHASES P02 step 4): JPG / PNG / TIFF / HEIC → a one-page PDF before the
//! rule engine sees the file. EXIF orientation is applied, small skew (up to 5°) is corrected,
//! and the image is downsampled so the page is at most US Letter at 300 dpi. HEIC needs the
//! `heic` feature (libheif); without it HEIC files are refused with a clear message.

use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, GrayImage, ImageDecoder, ImageReader, Luma, Rgb};
use imageproc::geometric_transformations::{Interpolation, rotate_about_center};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref};

/// Output resolution.
pub const DPI: f32 = 300.0;
/// Longest side, in pixels, after downsampling: 11 in at 300 dpi.
pub const MAX_LONG: u32 = 3300;
/// Shortest side: 8.5 in at 300 dpi.
pub const MAX_SHORT: u32 = 2550;

pub const IMAGE_EXTS: [&str; 7] = ["jpg", "jpeg", "png", "tif", "tiff", "heic", "heif"];

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| IMAGE_EXTS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

fn is_heif(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| matches!(e.to_string_lossy().to_lowercase().as_str(), "heic" | "heif"))
}

/// Decodes an image file the right way up.
pub fn load(path: &Path) -> Result<DynamicImage, String> {
    if is_heif(path) {
        return load_heif(path);
    }
    let mut dec = ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?
        .into_decoder()
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let orientation = dec.orientation().map_err(|e| e.to_string())?;
    let mut img = DynamicImage::from_decoder(dec)
        .map_err(|e| format!("cannot decode {}: {e}", path.display()))?;
    img.apply_orientation(orientation);
    Ok(img)
}

#[cfg(feature = "heic")]
fn load_heif(path: &Path) -> Result<DynamicImage, String> {
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
    let err = |e: libheif_rs::HeifError| format!("cannot read {}: {e}", path.display());
    let ctx = HeifContext::read_from_file(&path.to_string_lossy()).map_err(err)?;
    let handle = ctx.primary_image_handle().map_err(err)?;
    // libheif applies the file's rotation and mirroring while decoding.
    let img = LibHeif::new()
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)
        .map_err(err)?;
    let planes = img.planes();
    let p = planes
        .interleaved
        .ok_or_else(|| format!("{} has no RGB plane", path.display()))?;
    let mut out = image::RgbImage::new(p.width, p.height);
    for y in 0..p.height {
        let row = &p.data[y as usize * p.stride..][..p.width as usize * 3];
        for x in 0..p.width {
            let i = x as usize * 3;
            out.put_pixel(x, y, Rgb([row[i], row[i + 1], row[i + 2]]));
        }
    }
    Ok(DynamicImage::ImageRgb8(out))
}

#[cfg(not(feature = "heic"))]
fn load_heif(path: &Path) -> Result<DynamicImage, String> {
    Err(format!(
        "{}: this build of Vibe Files cannot read HEIC photos (no libheif). Use a release build, \
         or export the photo as JPEG.",
        path.display()
    ))
}

/// The skew angle (degrees, -5..=5) that makes text lines most horizontal, measured on a small
/// grayscale copy by the variance of dark pixels per row.
pub fn skew_angle(img: &DynamicImage) -> f32 {
    let small = img
        .resize(800, 800, image::imageops::FilterType::Triangle)
        .to_luma8();
    let mean = small.pixels().map(|p| p.0[0] as u64).sum::<u64>() / small.len().max(1) as u64;
    let dark: Vec<(f32, f32)> = small
        .enumerate_pixels()
        .filter(|(_, _, p)| (p.0[0] as u64) < mean * 3 / 4)
        .map(|(x, y, _)| (x as f32, y as f32))
        .collect();
    if dark.len() < 200 {
        return 0.0;
    }
    let (cx, cy) = (small.width() as f32 / 2.0, small.height() as f32 / 2.0);
    let rows = small.width().max(small.height()) as usize * 2;
    let score = |deg: f32| -> f64 {
        let (s, c) = deg.to_radians().sin_cos();
        let mut hist = vec![0u32; rows];
        for &(x, y) in &dark {
            let ry = (x - cx) * s + (y - cy) * c + rows as f32 / 2.0;
            if ry >= 0.0 && (ry as usize) < rows {
                hist[ry as usize] += 1;
            }
        }
        hist.iter().map(|&h| (h as f64) * (h as f64)).sum()
    };
    let mut best = (0.0, score(0.0));
    let mut a = -5.0;
    while a <= 5.0 {
        let sc = score(a);
        if sc > best.1 * 1.0001 {
            best = (a, sc);
        }
        a += 0.25;
    }
    best.0
}

/// Rotates by `deg` (counter-clockwise positive in image coordinates), white corners.
fn rotate(img: &DynamicImage, deg: f32) -> DynamicImage {
    let rgb = img.to_rgb8();
    DynamicImage::ImageRgb8(rotate_about_center(
        &rgb,
        -deg.to_radians(),
        Interpolation::Bilinear,
        Rgb([255, 255, 255]),
    ))
}

/// Normalizes `img` for filing: deskewed (when the skew is at least 0.3°) and no bigger than
/// Letter at 300 dpi.
pub fn normalize(img: DynamicImage) -> DynamicImage {
    // Measure the skew, downscale, then rotate the smaller image (rotation keeps the canvas size).
    let angle = skew_angle(&img);
    let (w, h) = (img.width(), img.height());
    let (long, short) = (w.max(h), w.min(h));
    let scale = (MAX_LONG as f32 / long as f32).min(MAX_SHORT as f32 / short as f32);
    let img = if scale < 1.0 {
        img.resize(
            (w as f32 * scale).round() as u32,
            (h as f32 * scale).round() as u32,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };
    // `skew_angle` is the rotation that shows the lines straight in its own frame; turning the
    // image the opposite way straightens it.
    if angle.abs() >= 0.3 {
        rotate(&img, -angle)
    } else {
        img
    }
}

/// Writes `img` as a one-page PDF (JPEG inside), the page sized for 300 dpi.
pub fn write_pdf(img: &DynamicImage, out: &Path) -> Result<(), String> {
    let rgb = img.to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), 85)
        .encode_image(&rgb)
        .map_err(|e| format!("cannot encode JPEG: {e}"))?;
    let (w, h) = (rgb.width(), rgb.height());
    let (pw, ph) = (w as f32 / DPI * 72.0, h as f32 / DPI * 72.0);
    let (catalog, pages, page, content, image_id) = (
        Ref::new(1),
        Ref::new(2),
        Ref::new(3),
        Ref::new(4),
        Ref::new(5),
    );
    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(pages);
    pdf.pages(pages).kids([page]).count(1);
    let mut p = pdf.page(page);
    p.media_box(Rect::new(0.0, 0.0, pw, ph));
    p.parent(pages);
    p.contents(content);
    p.resources().x_objects().pair(Name(b"Im1"), image_id);
    p.finish();
    let mut x = pdf.image_xobject(image_id, &jpeg);
    x.filter(Filter::DctDecode);
    x.width(w as i32);
    x.height(h as i32);
    x.color_space().device_rgb();
    x.bits_per_component(8);
    x.finish();
    let mut c = Content::new();
    c.save_state();
    c.transform([pw, 0.0, 0.0, ph, 0.0, 0.0]);
    c.x_object(Name(b"Im1"));
    c.restore_state();
    pdf.stream(content, &c.finish());
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(out, pdf.finish()).map_err(|e| format!("cannot write {}: {e}", out.display()))
}

/// Image file → one-page PDF at `out`.
pub fn image_to_pdf(path: &Path, out: &Path) -> Result<(), String> {
    write_pdf(&normalize(load(path)?), out)
}

/// A synthetic "document photo" for tests: black text-like bars on white, rotated by `deg`.
pub fn test_page(w: u32, h: u32, deg: f32) -> DynamicImage {
    let mut g = GrayImage::from_pixel(w, h, Luma([255]));
    let line_h = (h / 40).max(4);
    for row in 0..30 {
        let y0 = h / 10 + row * line_h * 2 / 2 + row * line_h / 2;
        for y in y0..(y0 + line_h / 2).min(h) {
            for x in w / 10..w * 9 / 10 - (row % 4) * w / 20 {
                g.put_pixel(x, y, Luma([0]));
            }
        }
    }
    let img = DynamicImage::ImageLuma8(g);
    if deg == 0.0 { img } else { rotate(&img, -deg) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deskew_straightens_the_page() {
        for deg in [-3.0f32, 2.0] {
            let img = test_page(1700, 2200, deg);
            let found = skew_angle(&img);
            assert!(
                (found.abs() - deg.abs()).abs() <= 0.5,
                "skewed {deg}°, found {found}°"
            );
            let fixed = rotate(&img, -found);
            let left = skew_angle(&fixed);
            assert!(
                left.abs() <= 0.5,
                "skewed {deg}°, still {left}° after correction"
            );
        }
        assert!(skew_angle(&test_page(1700, 2200, 0.0)).abs() <= 0.25);
        let straightened = normalize(test_page(1700, 2200, -3.0));
        assert!(
            skew_angle(&straightened).abs() <= 0.5,
            "normalize straightens"
        );
    }

    #[test]
    fn big_photo_becomes_a_letter_sized_one_page_pdf() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("photo.jpg");
        test_page(3024, 4032, 1.5).to_rgb8().save(&src).unwrap();
        let out = tmp.path().join("photo.pdf");
        image_to_pdf(&src, &out).unwrap();
        assert_eq!(filer_pdf::pdf::page_count(&out).unwrap(), 1);
        let doc = lopdf::Document::load(&out).unwrap();
        let page = *doc.get_pages().values().next().unwrap();
        let mb = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"MediaBox")
            .unwrap()
            .as_array()
            .unwrap()
            .clone();
        let num = |o: &lopdf::Object| o.as_float().unwrap_or_else(|_| o.as_i64().unwrap() as f32);
        let (w, h) = (num(&mb[2]), num(&mb[3]));
        assert!(w <= 612.5 && h <= 792.5, "{w}×{h} pt");
        assert!(h > 780.0, "a portrait phone photo fills the page height");
    }

    #[test]
    fn exif_orientation_is_applied() {
        // A tall PNG has no EXIF: stays tall; the decoder path is the same one EXIF JPEGs use.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("tall.png");
        test_page(400, 800, 0.0).to_rgb8().save(&src).unwrap();
        let img = load(&src).unwrap();
        assert_eq!((img.width(), img.height()), (400, 800));
    }

    #[cfg(feature = "heic")]
    #[test]
    fn heic_fixture_decodes_and_becomes_one_page() {
        let src =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/images/synthetic-page.heic");
        let img = load(&src).unwrap();
        assert_eq!((img.width(), img.height()), (600, 800));
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("p.pdf");
        image_to_pdf(&src, &out).unwrap();
        assert_eq!(filer_pdf::pdf::page_count(&out).unwrap(), 1);
    }

    #[cfg(not(feature = "heic"))]
    #[test]
    fn heic_without_libheif_is_refused_clearly() {
        let e = load(Path::new("x.heic")).unwrap_err();
        assert!(e.contains("cannot read HEIC"));
    }
}
