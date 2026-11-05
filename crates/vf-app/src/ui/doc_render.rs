//! The Viewer's worker thread (P04 step 1): one document loaded into memory (the file itself
//! is never held open, so Save can move the new version into place), its page geometry, page
//! images at any width and view rotation, find-in-document and the words of a page (text
//! selection, markup annotations).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use pdfium_render::prelude::{PdfDocument, PdfSearchOptions};
use vf_pdf::layer::Geometry;

use crate::ui::pages::{Rendered, render_doc};

pub use vf_pdf::text::WordBox;

/// What an opened document is.
#[derive(Debug, Clone)]
pub struct DocInfo {
    pub geoms: Vec<Geometry>,
    pub bytes: usize,
    /// Its fillable fields (PA-27).
    pub fields: Vec<vf_pdf::forms::Field>,
}

pub enum Job {
    Open {
        generation: u64,
        path: PathBuf,
    },
    Page {
        generation: u64,
        page: usize,
        width: u32,
        rotate: u16,
        terms: Vec<String>,
    },
    Find {
        generation: u64,
        text: String,
    },
    Words {
        generation: u64,
        page: usize,
    },
    Pii {
        generation: u64,
        kinds: Vec<vf_pdf::pii::Kind>,
    },
}

pub enum Done {
    Opened {
        generation: u64,
        info: Result<DocInfo, String>,
    },
    Page {
        generation: u64,
        page: usize,
        width: u32,
        rotate: u16,
        image: Result<Rendered, String>,
    },
    /// Pages with matches and how many on each.
    Found {
        generation: u64,
        text: String,
        pages: Vec<(usize, usize)>,
    },
    Words {
        generation: u64,
        page: usize,
        words: Vec<WordBox>,
    },
    Pii {
        generation: u64,
        found: Vec<vf_pdf::pii::Found>,
    },
}

pub struct DocRenderer {
    tx: Sender<Job>,
    rx: Receiver<Done>,
}

impl DocRenderer {
    pub fn new(ctx: &egui::Context) -> DocRenderer {
        let (tx, jobs) = channel::<Job>();
        let (out, rx) = channel();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("vf-viewer".into())
            .spawn(move || run(&jobs, &out, &ctx))
            .expect("spawn viewer renderer");
        DocRenderer { tx, rx }
    }

    pub fn send(&self, job: Job) {
        let _ = self.tx.send(job);
    }

    pub fn finished(&self) -> Vec<Done> {
        self.rx.try_iter().collect()
    }
}

fn run(jobs: &Receiver<Job>, out: &Sender<Done>, ctx: &egui::Context) {
    let pdfium = crate::ui::intake::pdfium_dir()
        .ok_or_else(|| "pdfium.dll not found".to_string())
        .and_then(|d| filer_pdf::pdf::pdfium(&d));
    let mut doc: Option<(u64, PdfDocument<'static>)> = None;
    let mut pending: Vec<Job> = Vec::new();
    loop {
        let job = match pending.pop() {
            Some(j) => j,
            None => match jobs.recv() {
                Ok(j) => j,
                Err(_) => return,
            },
        };
        // Page jobs piling up (fast scrolling): render the newest few, skip what scrolled by.
        let mut queued: Vec<Job> = jobs.try_iter().collect();
        if queued.len() > 8 {
            queued.drain(..queued.len() - 8);
        }
        pending.extend(queued.into_iter().rev());
        let done = match job {
            Job::Open { generation, path } => {
                let info = match &pdfium {
                    Err(e) => Err(e.clone()),
                    Ok(p) => std::fs::read(&path)
                        .map_err(|e| format!("cannot read {}: {e}", path.display()))
                        .and_then(|bytes| {
                            let n = bytes.len();
                            let (geoms, fields) = lopdf::Document::load_mem(&bytes)
                                .map(|d| {
                                    (vf_pdf::layer::geometries(&d), vf_pdf::forms::read_doc(&d))
                                })
                                .map_err(|e| format!("cannot read the PDF: {e}"))?;
                            let d = p
                                .load_pdf_from_byte_vec(bytes, None)
                                .map_err(|e| format!("cannot open the PDF: {e}"))?;
                            doc = Some((generation, d));
                            Ok(DocInfo {
                                geoms,
                                bytes: n,
                                fields,
                            })
                        }),
                };
                if info.is_err() {
                    doc = None;
                }
                Done::Opened { generation, info }
            }
            Job::Page {
                generation,
                page,
                width,
                rotate,
                terms,
            } => {
                let image = match &doc {
                    Some((g, d)) if *g == generation => render_doc(d, page, width, rotate, &terms),
                    _ => continue,
                };
                Done::Page {
                    generation,
                    page,
                    width,
                    rotate,
                    image,
                }
            }
            Job::Find { generation, text } => {
                let Some((g, d)) = &doc else { continue };
                if *g != generation {
                    continue;
                }
                Done::Found {
                    generation,
                    pages: find(d, &text),
                    text,
                }
            }
            Job::Pii { generation, kinds } => {
                let Some((g, d)) = &doc else { continue };
                if *g != generation {
                    continue;
                }
                Done::Pii {
                    generation,
                    found: pii(d, &kinds),
                }
            }
            Job::Words { generation, page } => {
                let Some((g, d)) = &doc else { continue };
                if *g != generation {
                    continue;
                }
                Done::Words {
                    generation,
                    page,
                    words: words(d, page),
                }
            }
        };
        if out.send(done).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

fn find(doc: &PdfDocument<'_>, text: &str) -> Vec<(usize, usize)> {
    if text.trim().is_empty() {
        return vec![];
    }
    let mut out = Vec::new();
    for (i, page) in doc.pages().iter().enumerate() {
        let Ok(t) = page.text() else { continue };
        let Ok(search) = t.search(text, &PdfSearchOptions::new()) else {
            continue;
        };
        let mut n = 0;
        while search.find_next().is_some() {
            n += 1;
        }
        if n > 0 {
            out.push((i, n));
        }
    }
    out
}

/// The page's words with their boxes (user space), in reading order.
pub fn words(doc: &PdfDocument<'_>, page: usize) -> Vec<WordBox> {
    doc.pages()
        .get(page as i32)
        .map(|p| vf_pdf::text::words(&p))
        .unwrap_or_default()
}

/// Every page's PII matches of `kinds`.
pub fn pii(doc: &PdfDocument<'_>, kinds: &[vf_pdf::pii::Kind]) -> Vec<vf_pdf::pii::Found> {
    doc.pages()
        .iter()
        .enumerate()
        .flat_map(|(i, p)| vf_pdf::pii::find_on_page(i, &vf_pdf::text::chars(&p), kinds))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_carry_their_boxes() {
        let t = tempfile::tempdir().unwrap();
        let pdf = t.path().join("a.pdf");
        filer_pdf::pdf::write_test_pdf(&pdf, &["Payer EIN 12-3456789"]);
        let dir = filer_pdf::pdf::find_pdfium(std::path::Path::new("")).unwrap();
        let doc = filer_pdf::pdf::pdfium(&dir)
            .unwrap()
            .load_pdf_from_byte_vec(std::fs::read(&pdf).unwrap(), None)
            .unwrap();
        let w = words(&doc, 0);
        let texts: Vec<&str> = w.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["Payer", "EIN", "12-3456789"]);
        assert!(w[0].rect[2] < w[1].rect[0], "left to right");
        assert!(w.iter().all(|w| w.rect[3] > w.rect[1]));
        assert_eq!(find(&doc, "3456"), vec![(0, 1)]);
        assert!(find(&doc, "nope").is_empty());
    }

    /// A 300-page scanned return: every page one 200 dpi gray scan (all pages share the image,
    /// the file stays small; pdfium still decodes it for the page).
    fn scanned_return(path: &std::path::Path, pages: usize) {
        use pdf_writer::{Content, Filter, Name, Pdf, Rect, Ref};
        let img = image::GrayImage::from_fn(1700, 2200, |x, y| {
            image::Luma([if (x / 40 + y / 25) % 7 == 0 { 30 } else { 245 }])
        });
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 70)
            .encode_image(&image::DynamicImage::ImageLuma8(img))
            .unwrap();
        let mut pdf = Pdf::new();
        let (catalog, tree, image, content) = (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4));
        pdf.catalog(catalog).pages(tree);
        let kids: Vec<Ref> = (0..pages).map(|i| Ref::new(10 + i as i32)).collect();
        pdf.pages(tree)
            .kids(kids.iter().copied())
            .count(pages as i32);
        let mut x = pdf.image_xobject(image, &jpeg);
        x.filter(Filter::DctDecode);
        x.width(1700);
        x.height(2200);
        x.color_space().device_gray();
        x.bits_per_component(8);
        drop(x);
        let mut c = Content::new();
        c.transform([612.0, 0.0, 0.0, 792.0, 0.0, 0.0]);
        c.x_object(Name(b"Im1"));
        pdf.stream(content, &c.finish());
        for id in kids {
            let mut p = pdf.page(id);
            p.media_box(Rect::new(0.0, 0.0, 612.0, 792.0));
            p.parent(tree);
            p.contents(content);
            p.resources().x_objects().pair(Name(b"Im1"), image);
        }
        std::fs::write(path, pdf.finish()).unwrap();
    }

    /// PHASES P04 done-when: a 300-page scanned return renders its first page in < 200 ms
    /// (read into memory, page geometry, open, first page at 640 px), and stepping to the
    /// next pages stays well inside a frame budget for a smooth scroll.
    #[test]
    fn three_hundred_page_scan_shows_its_first_page_fast() {
        let t = tempfile::tempdir().unwrap();
        let pdf = t.path().join("return.pdf");
        scanned_return(&pdf, 300);
        let dir = filer_pdf::pdf::find_pdfium(std::path::Path::new("")).unwrap();
        let pdfium = filer_pdf::pdf::pdfium(&dir).unwrap();
        // Warm the library once (the app does this at start).
        drop(
            pdfium
                .load_pdf_from_byte_vec(std::fs::read(&pdf).unwrap(), None)
                .unwrap(),
        );

        let start = std::time::Instant::now();
        let bytes = std::fs::read(&pdf).unwrap();
        let geoms = vf_pdf::layer::geometries(&lopdf::Document::load_mem(&bytes).unwrap());
        let doc = pdfium.load_pdf_from_byte_vec(bytes, None).unwrap();
        let first = render_doc(&doc, 0, 640, 0, &[]).unwrap();
        let ms = start.elapsed().as_millis();
        assert_eq!(geoms.len(), 300);
        assert_eq!(doc.pages().len(), 300);
        assert_eq!(first.image.size[0], 640);
        assert!(ms < 200, "first page took {ms} ms");

        let start = std::time::Instant::now();
        for p in 1..6 {
            render_doc(&doc, p, 640, 0, &[]).unwrap();
        }
        let per_page = start.elapsed().as_millis() / 5;
        assert!(per_page < 100, "{per_page} ms per page while scrolling");
    }
}
