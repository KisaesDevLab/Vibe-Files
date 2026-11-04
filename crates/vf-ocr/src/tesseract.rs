//! Tesseract as a separate program: the page goes in as a PGM file, the words come back as
//! Tesseract's TSV (level-5 rows carry the box, confidence and text).

use std::path::{Path, PathBuf};
use std::process::Command;

use image::GrayImage;

use crate::engine::{OcrEngine, OcrError, OcrPage, Word};

#[derive(Debug, Clone)]
pub struct Tesseract {
    pub exe: PathBuf,
    /// Language(s), `eng` (Q-07: English only in v1).
    pub lang: String,
    /// `tessdata` folder when it is not where the exe expects it.
    pub tessdata: Option<PathBuf>,
}

/// Where the UB Mannheim installer puts it.
const PROGRAM_FILES: &str = r"C:\Program Files\Tesseract-OCR\tesseract.exe";

impl Tesseract {
    /// Finds `tesseract.exe`: `configured` (the exe or its folder), then `<exe dir>\tesseract\`,
    /// Program Files, `PATH`.
    pub fn locate(configured: Option<&Path>) -> Result<Tesseract, OcrError> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(c) = configured {
            candidates.push(if c.is_dir() {
                c.join("tesseract.exe")
            } else {
                c.to_path_buf()
            });
        }
        if let Some(dir) = vf_core::exe_dir() {
            candidates.push(dir.join("tesseract").join("tesseract.exe"));
        }
        candidates.push(PathBuf::from(PROGRAM_FILES));
        if let Some(path) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&path).map(|d| d.join("tesseract.exe")));
        }
        let exe = candidates.into_iter().find(|p| p.is_file()).ok_or_else(|| {
            OcrError::Unavailable(
                "tesseract.exe not found. Install Tesseract, or put it in a `tesseract` folder beside vibe-files.exe (Admin › OCR).".into(),
            )
        })?;
        let tessdata = exe
            .parent()
            .map(|d| d.join("tessdata"))
            .filter(|d| d.is_dir());
        Ok(Tesseract {
            exe,
            lang: "eng".into(),
            tessdata,
        })
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.exe);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            c.creation_flags(CREATE_NO_WINDOW);
        }
        c
    }
}

/// The longest one page may take (a stuck or looping Tesseract is stopped; QA I10).
pub const PAGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Page images left by a run that stopped mid-page (they hold client pages): removed once per
/// start when older than an hour (QA I14).
fn sweep_temp(dir: &Path) {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        let hour = std::time::Duration::from_secs(3600);
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|a| a > hour);
            if n.starts_with("vf-ocr-") && n.ends_with(".pgm") && old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    });
}

/// Runs `cmd` with its output collected, killing it after `limit`.
fn output_within(
    cmd: &mut Command,
    limit: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Read both pipes on their own threads so a full pipe never stalls the child.
    let mut so = child.stdout.take().expect("piped");
    let mut se = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("stopped after {} s", limit.as_secs()),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    Ok(std::process::Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

impl OcrEngine for Tesseract {
    fn name(&self) -> &str {
        "tesseract"
    }

    fn recognize(&self, page: &GrayImage, dpi: u32) -> Result<OcrPage, OcrError> {
        let temp = std::env::temp_dir();
        sweep_temp(&temp);
        let tmp = temp.join(format!("vf-ocr-{}.pgm", uuid::Uuid::now_v7().simple()));
        let mut pgm = format!("P5\n{} {}\n255\n", page.width(), page.height()).into_bytes();
        pgm.extend_from_slice(page.as_raw());
        std::fs::write(&tmp, pgm).map_err(|e| OcrError::Failed(format!("temp page: {e}")))?;
        let mut cmd = self.command();
        cmd.arg(&tmp).arg("stdout").args([
            "-l",
            &self.lang,
            "--psm",
            "3",
            "--dpi",
            &dpi.to_string(),
        ]);
        if let Some(d) = &self.tessdata {
            cmd.arg("--tessdata-dir").arg(d);
        }
        cmd.arg("tsv");
        let out = output_within(&mut cmd, PAGE_TIMEOUT);
        let _ = std::fs::remove_file(&tmp);
        let out = out.map_err(|e| match e.kind() {
            // A page Tesseract cannot finish fails this document, not the engine.
            std::io::ErrorKind::TimedOut => OcrError::Failed(format!("Tesseract {e}")),
            _ => OcrError::Unavailable(format!("{}: {e}", self.exe.display())),
        })?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(OcrError::Failed(
                err.lines()
                    .last()
                    .unwrap_or("tesseract failed")
                    .trim()
                    .to_string(),
            ));
        }
        let mut p = parse_tsv(&String::from_utf8_lossy(&out.stdout));
        p.width = page.width();
        p.height = page.height();
        Ok(p)
    }
}

/// Tesseract TSV → words in reading order. Columns: level page_num block_num par_num line_num
/// word_num left top width height conf text.
pub fn parse_tsv(tsv: &str) -> OcrPage {
    let mut words = Vec::new();
    let mut last_line = None;
    let mut line_no: u32 = 0;
    for row in tsv.lines().skip(1) {
        let c: Vec<&str> = row.split('\t').collect();
        if c.len() < 12 || c[0] != "5" {
            continue;
        }
        let text = c[11].trim();
        if text.is_empty() {
            continue;
        }
        let n = |i: usize| c[i].trim().parse::<i64>().unwrap_or(0).max(0) as u32;
        let key = (n(2), n(3), n(4));
        if last_line.is_some_and(|k| k != key) {
            line_no += 1;
        }
        last_line = Some(key);
        words.push(Word {
            bbox: [n(6), n(7), n(8), n(9)],
            text: text.to_string(),
            conf: c[10].trim().parse().unwrap_or(0.0),
            line: line_no,
        });
    }
    OcrPage {
        width: 0,
        height: 0,
        words,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TSV: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext
1\t1\t0\t0\t0\t0\t0\t0\t2550\t3300\t-1\t
4\t1\t1\t1\t1\t0\t300\t400\t900\t40\t-1\t
5\t1\t1\t1\t1\t1\t300\t400\t120\t40\t96.5\tEIN
5\t1\t1\t1\t1\t2\t440\t400\t400\t40\t91.2\t12-3456789
5\t1\t1\t1\t1\t3\t860\t400\t10\t40\t95\t
5\t1\t1\t1\t2\t1\t300\t460\t200\t40\t88\tAcme
5\t1\t2\t1\t1\t1\t300\t900\t200\t40\t70\tLLC
";

    #[test]
    fn tsv_words_boxes_and_lines() {
        let p = parse_tsv(TSV);
        assert_eq!(p.words.len(), 4, "blank words dropped");
        assert_eq!(p.words[1].bbox, [440, 400, 400, 40]);
        assert_eq!(p.words[1].conf, 91.2);
        assert_eq!(p.text(), "EIN 12-3456789\nAcme\nLLC");
    }

    #[test]
    fn missing_program_is_unavailable_not_failed() {
        let t = Tesseract {
            exe: PathBuf::from(r"C:\nowhere\tesseract.exe"),
            lang: "eng".into(),
            tessdata: None,
        };
        let e = t.recognize(&GrayImage::new(10, 10), 300).unwrap_err();
        assert!(matches!(e, OcrError::Unavailable(_)), "{e}");
    }
}
