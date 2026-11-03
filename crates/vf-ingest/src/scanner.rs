//! Scanner (P02 step 2). WIA, through Windows' own scanner API (`Windows.Devices.Scanners`,
//! which drives WIA 2.0 drivers): no TWAIN data-source manager to ship, works from a 64-bit
//! exe, and the feeder, duplex, resolution and color settings are typed. QUESTIONS Q-02.
//!
//! A scan writes raw page images into a hidden work folder inside the Scans folder, then
//! [`assemble`] turns them into one PDF there (blank pages dropped by ink coverage, deskewed,
//! at most 300 dpi). The Scans folder worker takes it from there: barcode split, Filer's inbox.

use std::path::{Path, PathBuf};

use image::{DynamicImage, imageops};
use serde::{Deserialize, Serialize};

use crate::images::{self, Frame, Normalized};

/// Where the paper is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Document feeder (ADF).
    Feeder,
    Flatbed,
    /// Whatever the scanner picks.
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Color {
    Color,
    Gray,
    /// Black and white.
    Mono,
}

/// A scan profile. Kept per workstation in `config.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub source: Source,
    /// Both sides (feeder only).
    #[serde(default)]
    pub duplex: bool,
    pub dpi: u32,
    pub color: Color,
    #[serde(default = "yes")]
    pub deskew: bool,
    /// Drop pages whose ink coverage is under `blank_ink`.
    #[serde(default = "yes")]
    pub drop_blank: bool,
    /// Share of dark pixels (0–1) under which a page is blank.
    #[serde(default = "blank_ink")]
    pub blank_ink: f32,
}

fn yes() -> bool {
    true
}
fn blank_ink() -> f32 {
    BLANK_INK
}

/// Default blank threshold. Higher than the barcode split's (a scanned "blank" page carries
/// speckle and bleed-through from the other side).
pub const BLANK_INK: f32 = 0.004;

/// Profiles offered when the workstation has none of its own.
pub fn default_profiles() -> Vec<Profile> {
    let p = |name: &str, source, duplex, dpi, color| Profile {
        name: name.into(),
        source,
        duplex,
        dpi,
        color,
        deskew: true,
        drop_blank: true,
        blank_ink: BLANK_INK,
    };
    vec![
        p(
            "Stack, both sides (gray 300)",
            Source::Feeder,
            true,
            300,
            Color::Gray,
        ),
        p(
            "Stack, one side (gray 300)",
            Source::Feeder,
            false,
            300,
            Color::Gray,
        ),
        p(
            "Stack, both sides (color 200)",
            Source::Feeder,
            true,
            200,
            Color::Color,
        ),
        p(
            "Flatbed (color 300)",
            Source::Flatbed,
            false,
            300,
            Color::Color,
        ),
    ]
}

// ---------------------------------------------------------------------------------------------
// Assembly (no scanner needed)

/// What one scan produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Assembled {
    /// Pages scanned.
    pub scanned: usize,
    /// Pages dropped as blank.
    pub blank: usize,
    pub normalized: Normalized,
}

impl Assembled {
    pub fn note(&self) -> String {
        let mut s = self.normalized.note();
        if self.blank > 0 {
            s.push_str(&format!(" · {} blank dropped", self.blank));
        }
        s
    }
}

/// Ink coverage of a scanned page, ignoring a 4 % margin (feeder edge shadows).
pub fn page_ink(rgb: &image::RgbImage) -> f32 {
    let small = DynamicImage::ImageRgb8(rgb.clone())
        .resize(800, 800, imageops::FilterType::Triangle)
        .to_luma8();
    let (w, h) = small.dimensions();
    let (mx, my) = (w / 25, h / 25);
    if w <= 2 * mx || h <= 2 * my {
        return crate::stack::ink(small.as_raw());
    }
    let inner = imageops::crop_imm(&small, mx, my, w - 2 * mx, h - 2 * my).to_image();
    crate::stack::ink(inner.as_raw())
}

/// Turns the page images of one scan, in order, into one PDF at `out`.
pub fn assemble(pages: &[PathBuf], profile: &Profile, out: &Path) -> Result<Assembled, String> {
    let mut frames: Vec<Frame> = Vec::new();
    for p in pages {
        frames.extend(images::decode(p)?);
    }
    let scanned = frames.len();
    if profile.drop_blank {
        frames.retain(|f| page_ink(&f.rgb) >= profile.blank_ink);
    }
    let blank = scanned - frames.len();
    if frames.is_empty() {
        return Err(format!("all {scanned} scanned pages were blank"));
    }
    let normalized = images::frames_to_pdf(frames, profile.deskew, out)?;
    Ok(Assembled {
        scanned,
        blank,
        normalized,
    })
}

// ---------------------------------------------------------------------------------------------
// WIA

/// A scanner Windows knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
}

fn err(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |e| format!("{what}: {}", e.message())
}

/// The scanners on this workstation (WIA drivers).
pub fn devices() -> Result<Vec<Device>, String> {
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Devices::Scanners::ImageScanner;
    let selector = ImageScanner::GetDeviceSelector().map_err(err("scanner query"))?;
    let all = DeviceInformation::FindAllAsyncAqsFilter(&selector)
        .and_then(|op| op.join())
        .map_err(err("cannot list scanners"))?;
    let mut out = Vec::new();
    for i in 0..all.Size().map_err(err("cannot list scanners"))? {
        let d = all.GetAt(i).map_err(err("cannot list scanners"))?;
        out.push(Device {
            id: d.Id().map_err(err("scanner id"))?.to_string(),
            name: d.Name().map_err(err("scanner name"))?.to_string(),
        });
    }
    Ok(out)
}

/// Scans with `profile` into `dir` and returns the page image files, in page order.
pub fn scan_pages(device_id: &str, profile: &Profile, dir: &Path) -> Result<Vec<PathBuf>, String> {
    use windows::Devices::Scanners::{
        ImageScanner, ImageScannerColorMode, ImageScannerFormat, ImageScannerResolution,
        ImageScannerScanSource,
    };
    use windows::Storage::StorageFolder;
    use windows::core::HSTRING;

    let scanner = ImageScanner::FromIdAsync(&HSTRING::from(device_id))
        .and_then(|op| op.join())
        .map_err(err("cannot open the scanner"))?;
    let source = match profile.source {
        Source::Feeder => ImageScannerScanSource::Feeder,
        Source::Flatbed => ImageScannerScanSource::Flatbed,
        Source::Auto => ImageScannerScanSource::AutoConfigured,
    };
    if !scanner
        .IsScanSourceSupported(source)
        .map_err(err("scanner"))?
    {
        return Err(format!(
            "this scanner has no {}",
            match profile.source {
                Source::Feeder => "document feeder",
                Source::Flatbed => "flatbed",
                Source::Auto => "automatic mode",
            }
        ));
    }
    let mode = match profile.color {
        Color::Color => ImageScannerColorMode::Color,
        Color::Gray => ImageScannerColorMode::Grayscale,
        Color::Mono => ImageScannerColorMode::Monochrome,
    };
    let res = ImageScannerResolution {
        DpiX: profile.dpi as f32,
        DpiY: profile.dpi as f32,
    };
    // Lossless first: the pages are compressed once, when the PDF is written.
    let formats = [
        ImageScannerFormat::Png,
        ImageScannerFormat::Tiff,
        ImageScannerFormat::Jpeg,
    ];
    macro_rules! configure {
        ($c:expr) => {{
            let c = $c.map_err(err("scanner settings"))?;
            if c.IsColorModeSupported(mode)
                .map_err(err("scanner settings"))?
            {
                c.SetColorMode(mode).map_err(err("color mode"))?;
            }
            c.SetDesiredResolution(res).map_err(err("resolution"))?;
            if let Some(f) = formats
                .iter()
                .find(|f| c.IsFormatSupported(**f).unwrap_or(false))
            {
                c.SetFormat(*f).map_err(err("file format"))?;
            }
            c
        }};
    }
    match profile.source {
        Source::Feeder => {
            let c = configure!(scanner.FeederConfiguration());
            if profile.duplex {
                if !c.CanScanDuplex().map_err(err("duplex"))? {
                    return Err("this scanner's feeder cannot scan both sides".into());
                }
                c.SetDuplex(true).map_err(err("duplex"))?;
            }
            c.SetMaxNumberOfPages(0).map_err(err("page count"))?;
        }
        Source::Flatbed => {
            configure!(scanner.FlatbedConfiguration());
        }
        Source::Auto => {}
    }

    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let abs = std::path::absolute(dir).map_err(|e| e.to_string())?;
    let folder = StorageFolder::GetFolderFromPathAsync(&HSTRING::from(abs.as_os_str()))
        .and_then(|op| op.join())
        .map_err(err("scan folder"))?;
    let result = scanner
        .ScanFilesToFolderAsync(source, &folder)
        .and_then(|op| op.join())
        .map_err(err("scan failed (is there paper in the feeder?)"))?;
    let files = result.ScannedFiles().map_err(err("scanned files"))?;
    let mut out = Vec::new();
    for i in 0..files.Size().map_err(err("scanned files"))? {
        let f = files.GetAt(i).map_err(err("scanned files"))?;
        out.push(PathBuf::from(
            f.Path().map_err(err("scanned file"))?.to_os_string(),
        ));
    }
    if out.is_empty() {
        return Err("the scanner returned no pages".into());
    }
    Ok(out)
}

/// Scans with `profile` and leaves one PDF in `scans_dir` for the Scans folder worker.
/// Returns its path and what was done.
pub fn scan_to(
    device_id: &str,
    profile: &Profile,
    scans_dir: &Path,
) -> Result<(PathBuf, Assembled), String> {
    // Hidden from the worker (it skips dot names and folders).
    let work = scans_dir.join(format!(".scan-{}", uuid::Uuid::now_v7().simple()));
    let r = scan_pages(device_id, profile, &work).and_then(|pages| {
        let out = crate::pipeline::free_name(scans_dir, &format!("Scan {}.pdf", stamp()));
        let tmp = scans_dir.join(format!(".scan-{}.pdf.tmp", uuid::Uuid::now_v7().simple()));
        let a = assemble(&pages, profile, &tmp)?;
        std::fs::rename(&tmp, &out).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot write {}: {e}", out.display())
        })?;
        Ok((out, a))
    });
    let _ = std::fs::remove_dir_all(&work);
    r
}

/// Date and time (UTC) for a file name, `2026-11-03 141502`.
fn stamp() -> String {
    let now = vf_core::now_rfc3339();
    now.get(..19)
        .unwrap_or("undated")
        .replace('T', " ")
        .replace(':', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn page(text: bool) -> RgbImage {
        let mut p = RgbImage::from_pixel(850, 1100, Rgb([250, 250, 250]));
        // Feeder shadow along the edge: not ink.
        for y in 0..1100 {
            for x in 0..12 {
                p.put_pixel(x, y, Rgb([40, 40, 40]));
            }
        }
        if text {
            for line in 0..30 {
                for y in 100 + line * 30..108 + line * 30 {
                    for x in 100..750 {
                        p.put_pixel(x, y, Rgb([10, 10, 10]));
                    }
                }
            }
        }
        p
    }

    #[test]
    fn blank_pages_are_dropped_by_ink() {
        assert!(page_ink(&page(false)) < BLANK_INK, "edge shadow is not ink");
        assert!(page_ink(&page(true)) > BLANK_INK);

        let t = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = [true, false, true, false, true]
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let p = t.path().join(format!("page{i}.png"));
                page(*text).save(&p).unwrap();
                p
            })
            .collect();
        let profile = &default_profiles()[0];
        let out = t.path().join("scan.pdf");
        let a = assemble(&files, profile, &out).unwrap();
        assert_eq!((a.scanned, a.blank, a.normalized.pages), (5, 2, 3));
        assert_eq!(a.note(), "3 pages · 2 blank dropped");
        assert_eq!(lopdf::Document::load(&out).unwrap().get_pages().len(), 3);

        let keep = Profile {
            drop_blank: false,
            ..profile.clone()
        };
        assert_eq!(assemble(&files, &keep, &out).unwrap().normalized.pages, 5);
        let only_blank = [files[1].clone()];
        assert!(assemble(&only_blank, profile, &out).is_err());
    }

    #[test]
    fn profiles_round_trip_with_defaults() {
        let p: Profile =
            toml::from_str("name='x'\nsource='feeder'\ndpi=300\ncolor='gray'").unwrap();
        assert!(p.deskew && p.drop_blank && !p.duplex);
        assert_eq!(p.blank_ink, BLANK_INK);
        let back: Profile =
            toml::from_str(&toml::to_string(&default_profiles()[0]).unwrap()).unwrap();
        assert_eq!(back, default_profiles()[0]);
    }

    /// Lists this workstation's scanners without failing when there are none (CI, laptops).
    #[test]
    fn device_list_works_with_or_without_a_scanner() {
        let d = devices().unwrap();
        assert!(d.iter().all(|d| !d.id.is_empty()));
    }
}
