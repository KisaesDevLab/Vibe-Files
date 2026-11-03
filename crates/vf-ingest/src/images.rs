//! Image normalization (P02 step 4): JPG / PNG / TIFF / BMP / HEIC → PDF, one page per image
//! (every frame of a multi-page TIFF), turned upright from the EXIF orientation, deskewed,
//! and at most 300 dpi.
//!
//! Decoding goes through Windows Imaging Component, so HEIC uses the HEIF codec Windows
//! already has (Microsoft's "HEIF Image Extensions"); no native library ships with the exe.
//! QUESTIONS Q-24 records this instead of `libheif-rs`.
//!
//! Page size: an image that says its resolution (a scanner's TIFF, 100–1200 dpi) keeps its
//! physical size; anything else (phone photos) is fitted to a Letter page, portrait or
//! landscape by its shape.

use std::path::Path;

use image::{DynamicImage, GrayImage, RgbImage, imageops};
use pdf_writer::{Content, Filter, Name, Pdf, Rect, Ref};

/// Extensions the Scans folder turns into PDFs.
pub const EXTENSIONS: [&str; 8] = ["jpg", "jpeg", "png", "tif", "tiff", "bmp", "heic", "heif"];
const MAX_DPI: f32 = 300.0;
const LETTER: (f32, f32) = (8.5, 11.0);
/// Deskew search range and step, degrees.
const SKEW_MAX: f32 = 5.0;
const SKEW_STEP: f32 = 0.25;

pub fn is_image(p: &Path) -> bool {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

/// One decoded frame: pixels (already upright), resolution if the file states a useful one,
/// and the EXIF orientation that was applied (1 = none).
pub struct Frame {
    pub rgb: RgbImage,
    pub dpi: Option<f32>,
    pub orientation: u8,
}

/// What [`to_pdf`] did, for the Intake strip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Normalized {
    pub pages: usize,
    pub rotated: bool,
    pub deskewed: bool,
    pub downsampled: bool,
}

impl Normalized {
    pub fn note(&self) -> String {
        let mut v = vec![if self.pages == 1 {
            "1 page".to_string()
        } else {
            format!("{} pages", self.pages)
        }];
        if self.rotated {
            v.push("EXIF rotate".into());
        }
        if self.deskewed {
            v.push("deskew".into());
        }
        if self.downsampled {
            v.push("300 dpi".into());
        }
        v.join(" · ")
    }
}

/// Converts the image at `src` into a PDF at `out` (replaced if it exists).
pub fn to_pdf(src: &Path, out: &Path) -> Result<Normalized, String> {
    let frames = decode(src)?;
    let mut n = Normalized {
        pages: frames.len(),
        ..Default::default()
    };
    let mut pages = Vec::new();
    for f in frames {
        n.rotated |= f.orientation > 1;
        let (rgb, deskewed) = deskew(f.rgb);
        n.deskewed |= deskewed;
        let (w_in, h_in) = page_inches(rgb.width(), rgb.height(), f.dpi);
        let cap = ((w_in * MAX_DPI) as u32, (h_in * MAX_DPI) as u32);
        let rgb = if rgb.width() > cap.0 || rgb.height() > cap.1 {
            n.downsampled = true;
            let s = (cap.0 as f32 / rgb.width() as f32).min(cap.1 as f32 / rgb.height() as f32);
            imageops::resize(
                &rgb,
                ((rgb.width() as f32 * s).round() as u32).max(1),
                ((rgb.height() as f32 * s).round() as u32).max(1),
                imageops::FilterType::CatmullRom,
            )
        } else {
            rgb
        };
        pages.push((rgb, w_in * 72.0, h_in * 72.0));
    }
    write_pdf(out, &pages)?;
    Ok(n)
}

/// The page an image goes on, in inches: its own size at its stated dpi, or Letter.
fn page_inches(w: u32, h: u32, dpi: Option<f32>) -> (f32, f32) {
    match dpi {
        Some(d) => (w as f32 / d, h as f32 / d),
        None if w > h => (LETTER.1, LETTER.0),
        None => LETTER,
    }
}

/// Writes one page per image, each image filling its page (aspect kept, centred), as JPEG.
fn write_pdf(out: &Path, pages: &[(RgbImage, f32, f32)]) -> Result<(), String> {
    let mut pdf = Pdf::new();
    let (catalog, tree) = (Ref::new(1), Ref::new(2));
    let mut next = 3;
    let mut kids = Vec::new();
    let mut objs = Vec::new();
    for _ in pages {
        objs.push((Ref::new(next), Ref::new(next + 1), Ref::new(next + 2)));
        kids.push(Ref::new(next));
        next += 3;
    }
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids(kids.iter().copied())
        .count(kids.len() as i32);
    for ((rgb, pw, ph), (page, content, img)) in pages.iter().zip(objs) {
        let gray = is_gray(rgb);
        let mut jpeg = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85);
        let r = if gray {
            enc.encode_image(&DynamicImage::ImageRgb8(rgb.clone()).to_luma8())
        } else {
            enc.encode_image(rgb)
        };
        r.map_err(|e| format!("cannot encode the page image: {e}"))?;
        let mut x = pdf.image_xobject(img, &jpeg);
        x.filter(Filter::DctDecode);
        x.width(rgb.width() as i32);
        x.height(rgb.height() as i32);
        if gray {
            x.color_space().device_gray();
        } else {
            x.color_space().device_rgb();
        }
        x.bits_per_component(8);
        drop(x);

        let s = (pw / rgb.width() as f32).min(ph / rgb.height() as f32);
        let (iw, ih) = (rgb.width() as f32 * s, rgb.height() as f32 * s);
        let mut c = Content::new();
        c.save_state();
        c.transform([iw, 0.0, 0.0, ih, (pw - iw) / 2.0, (ph - ih) / 2.0]);
        c.x_object(Name(b"Im1"));
        c.restore_state();
        pdf.stream(content, &c.finish());
        let mut p = pdf.page(page);
        p.media_box(Rect::new(0.0, 0.0, *pw, *ph));
        p.parent(tree);
        p.contents(content);
        p.resources().x_objects().pair(Name(b"Im1"), img);
    }
    vf_fs::cabinet::write_replace(out, &pdf.finish())
}

/// Nearly every pixel has r ≈ g ≈ b.
fn is_gray(rgb: &RgbImage) -> bool {
    let step = (rgb.len() / 3 / 20_000).max(1);
    let colored = rgb
        .pixels()
        .step_by(step)
        .filter(|p| {
            let [r, g, b] = p.0;
            r.abs_diff(g) > 12 || g.abs_diff(b) > 12 || r.abs_diff(b) > 12
        })
        .count();
    colored * 200 < rgb.len() / 3 / step
}

// ---------------------------------------------------------------------------------------------
// Deskew

/// Straightens a page photographed or scanned slightly crooked (within ±5°). Returns the
/// image and whether it was turned.
pub fn deskew(rgb: RgbImage) -> (RgbImage, bool) {
    let small = DynamicImage::ImageRgb8(rgb.clone())
        .resize(800, 800, imageops::FilterType::Triangle)
        .to_luma8();
    match skew_angle(&small) {
        Some(a) => (rotate(&rgb, -a), true),
        None => (rgb, false),
    }
}

/// The angle (degrees, counter-clockwise positive) the text lines of `gray` lean at, when it
/// is clearly not level. `None` for level pages and for pictures that are not text.
pub fn skew_angle(gray: &GrayImage) -> Option<f32> {
    let dark: Vec<(f32, f32)> = gray
        .enumerate_pixels()
        .filter(|(_, _, p)| p.0[0] < 128)
        .map(|(x, y, _)| (x as f32, y as f32))
        .collect();
    let share = dark.len() as f32 / (gray.width() * gray.height()).max(1) as f32;
    if !(0.002..=0.25).contains(&share) {
        return None; // blank, or a photo rather than a page of text
    }
    let score = |deg: f32| -> f64 {
        let (s, c) = deg.to_radians().sin_cos();
        let mut bins = vec![0u32; (gray.width() + gray.height()) as usize * 2];
        let off = gray.width() as f32;
        for &(x, y) in &dark {
            // Row of the point once the page is turned back by `deg`.
            let r = (y * c + x * s + off) as usize;
            if let Some(b) = bins.get_mut(r) {
                *b += 1;
            }
        }
        bins.iter().map(|&b| (b as f64) * (b as f64)).sum()
    };
    let steps = (SKEW_MAX / SKEW_STEP) as i32;
    let (best, best_score) = (-steps..=steps)
        .map(|i| i as f32 * SKEW_STEP)
        .map(|a| (a, score(a)))
        .fold(
            (0.0, f64::MIN),
            |m, (a, s)| if s > m.1 { (a, s) } else { m },
        );
    // Only when clearly better than leaving it level.
    (best.abs() >= 0.5 && best_score > score(0.0) * 1.05).then_some(best)
}

/// Turns `img` about its centre by `deg` (counter-clockwise positive), same size, white fill.
pub fn rotate(img: &RgbImage, deg: f32) -> RgbImage {
    let (w, h) = img.dimensions();
    let (s, c) = deg.to_radians().sin_cos();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let mut out = RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255]));
    for (x, y, px) in out.enumerate_pixels_mut() {
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        // Source point (inverse rotation; y grows downwards).
        let sx = c * dx - s * dy + cx - 0.5;
        let sy = s * dx + c * dy + cy - 0.5;
        if sx < 0.0 || sy < 0.0 || sx > (w - 1) as f32 || sy > (h - 1) as f32 {
            continue;
        }
        let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
        let p = |x, y| img.get_pixel(x, y).0;
        let (a, b, cc, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
        for k in 0..3 {
            let top = a[k] as f32 * (1.0 - fx) + b[k] as f32 * fx;
            let bot = cc[k] as f32 * (1.0 - fx) + d[k] as f32 * fx;
            px.0[k] = (top * (1.0 - fy) + bot * fy).round() as u8;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Windows Imaging Component

/// Every frame of the image file, upright.
pub fn decode(path: &Path) -> Result<Vec<Frame>, String> {
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;
    use windows::core::HSTRING;

    let name = path.display().to_string();
    // SAFETY: COM calls on this thread with interfaces obtained from WIC; buffers are sized
    // from the frame dimensions WIC reports.
    unsafe {
        // Any apartment works for WIC; "already initialized differently" is fine too.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| format!("Windows imaging is not available: {e}"))?;
        let decoder = factory
            .CreateDecoderFromFilename(
                &HSTRING::from(path.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(|e| {
                let heic = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("heic") || e.eq_ignore_ascii_case("heif"));
                if heic {
                    format!(
                        "{name}: Windows cannot read HEIC photos on this PC. Install \"HEIF Image Extensions\" (and \"HEVC Video Extensions\") from the Microsoft Store. ({e})"
                    )
                } else {
                    format!("{name}: not an image Windows can read ({e})")
                }
            })?;
        let count = decoder.GetFrameCount().map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for i in 0..count {
            let frame = decoder.GetFrame(i).map_err(|e| e.to_string())?;
            let orientation = frame
                .GetMetadataQueryReader()
                .ok()
                .and_then(|r| orientation(&r))
                .unwrap_or(1);
            let (mut dx, mut dy) = (0.0, 0.0);
            let dpi = frame
                .GetResolution(&mut dx, &mut dy)
                .ok()
                .map(|_| dx as f32)
                .filter(|d| (100.0..=1200.0).contains(d) && dx == dy);
            let conv = factory.CreateFormatConverter().map_err(|e| e.to_string())?;
            conv.Initialize(
                &frame,
                &GUID_WICPixelFormat24bppRGB,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(|e| format!("{name}: cannot convert frame {}: {e}", i + 1))?;
            let (mut w, mut h) = (0, 0);
            conv.GetSize(&mut w, &mut h).map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; w as usize * h as usize * 3];
            conv.CopyPixels(std::ptr::null(), w * 3, &mut buf)
                .map_err(|e| format!("{name}: cannot read frame {}: {e}", i + 1))?;
            let rgb = RgbImage::from_raw(w, h, buf).ok_or("frame size mismatch")?;
            let mut img = DynamicImage::ImageRgb8(rgb);
            if let Some(o) = image::metadata::Orientation::from_exif(orientation) {
                img.apply_orientation(o);
            }
            out.push(Frame {
                rgb: img.into_rgb8(),
                // A quarter turn swaps the axes; equal x/y resolution keeps the dpi valid.
                dpi,
                orientation,
            });
        }
        if out.is_empty() {
            return Err(format!("{name}: no image in the file"));
        }
        Ok(out)
    }
}

/// `System.Photo.Orientation` (EXIF 1–8) when the file has one.
unsafe fn orientation(
    r: &windows::Win32::Graphics::Imaging::IWICMetadataQueryReader,
) -> Option<u8> {
    use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
    use windows::Win32::System::Variant::VT_UI2;
    use windows::core::w;
    let mut v = PROPVARIANT::default();
    // SAFETY: `v` is a valid PROPVARIANT; it is cleared before it goes out of scope.
    unsafe {
        r.GetMetadataByName(w!("System.Photo.Orientation"), &mut v)
            .ok()?;
        let inner = &v.Anonymous.Anonymous;
        let o = (inner.vt == VT_UI2).then(|| inner.Anonymous.uiVal);
        let _ = PropVariantClear(&mut v);
        o.and_then(|o| u8::try_from(o).ok())
            .filter(|o| (1..=8).contains(o))
    }
}

/// Writes `frames` as an image file with Windows' own encoder (`jpeg`, `png`, `tiff`, `heif`),
/// optionally tagged with an EXIF orientation. For tests: real cameras and phones write the
/// files Intake gets.
#[doc(hidden)]
pub fn write_with_wic(
    path: &Path,
    format: &str,
    frames: &[RgbImage],
    orientation: Option<u16>,
) -> Result<(), String> {
    use windows::Win32::Foundation::GENERIC_WRITE;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::*;
    use windows::Win32::System::Variant::VT_UI2;
    use windows::core::{HSTRING, w};
    let container = match format {
        "jpeg" => GUID_ContainerFormatJpeg,
        "png" => GUID_ContainerFormatPng,
        "tiff" => GUID_ContainerFormatTiff,
        "heif" => GUID_ContainerFormatHeif,
        _ => return Err(format!("unknown format {format}")),
    };
    let e = |e: windows::core::Error| e.to_string();
    // SAFETY: COM calls with interfaces WIC returned; pixel buffers outlive the calls.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(e)?;
        let stream = factory.CreateStream().map_err(e)?;
        stream
            .InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)
            .map_err(e)?;
        let encoder = factory
            .CreateEncoder(&container, std::ptr::null())
            .map_err(e)?;
        encoder
            .Initialize(&stream, WICBitmapEncoderNoCache)
            .map_err(e)?;
        for rgb in frames {
            let (mut fe, mut bag) = (None, None);
            encoder.CreateNewFrame(&mut fe, &mut bag).map_err(e)?;
            let fe = fe.ok_or("no frame")?;
            fe.Initialize(bag.as_ref()).map_err(e)?;
            fe.SetSize(rgb.width(), rgb.height()).map_err(e)?;
            let mut fmt = GUID_WICPixelFormat24bppRGB;
            fe.SetPixelFormat(&mut fmt).map_err(e)?;
            let bitmap = factory
                .CreateBitmapFromMemory(
                    rgb.width(),
                    rgb.height(),
                    &GUID_WICPixelFormat24bppRGB,
                    rgb.width() * 3,
                    rgb.as_raw(),
                )
                .map_err(e)?;
            let conv = factory.CreateFormatConverter().map_err(e)?;
            conv.Initialize(
                &bitmap,
                &fmt,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(e)?;
            if let Some(o) = orientation {
                let mut v = PROPVARIANT::default();
                (*v.Anonymous.Anonymous).vt = VT_UI2;
                (*v.Anonymous.Anonymous).Anonymous.uiVal = o;
                fe.GetMetadataQueryWriter()
                    .and_then(|m| m.SetMetadataByName(w!("System.Photo.Orientation"), &v))
                    .map_err(e)?;
            }
            fe.WriteSource(&conv, std::ptr::null()).map_err(e)?;
            fe.Commit().map_err(e)?;
        }
        encoder.Commit().map_err(e)?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A white page with dark horizontal text-like bars, turned by `deg`.
    pub fn text_page(deg: f32) -> RgbImage {
        let mut img = RgbImage::from_pixel(850, 1100, image::Rgb([255, 255, 255]));
        for line in 0..30 {
            let y = 100 + line * 30;
            for x in 100..750 {
                if (x / 9) % 5 != 4 {
                    for dy in 0..8 {
                        img.put_pixel(x, y + dy, image::Rgb([20, 20, 20]));
                    }
                }
            }
        }
        if deg == 0.0 { img } else { rotate(&img, deg) }
    }

    #[test]
    fn deskew_finds_and_undoes_a_tilt() {
        let small = |img: &RgbImage| {
            DynamicImage::ImageRgb8(img.clone())
                .resize(800, 800, imageops::FilterType::Triangle)
                .to_luma8()
        };
        assert_eq!(skew_angle(&small(&text_page(0.0))), None, "level page");
        let tilted = text_page(3.0);
        let a = skew_angle(&small(&tilted)).expect("tilt found");
        assert!((a - 3.0).abs() <= 0.5, "found {a}");
        let (fixed, turned) = deskew(tilted);
        assert!(turned);
        assert_eq!(skew_angle(&small(&fixed)), None, "level after deskew");
        // A blank page is left alone.
        let blank = RgbImage::from_pixel(400, 500, image::Rgb([255, 255, 255]));
        assert!(!deskew(blank).1);
    }

    #[test]
    fn page_size_from_dpi_or_letter() {
        assert_eq!(page_inches(2550, 3300, Some(300.0)), (8.5, 11.0));
        assert_eq!(page_inches(4032, 3024, None), (11.0, 8.5));
        assert_eq!(page_inches(3024, 4032, None), (8.5, 11.0));
    }
}
