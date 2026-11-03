//! Page images for the Search results and the Viewer, rendered by pdfium on one worker
//! thread, with the places where the search words are on the page.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use pdfium_render::prelude::{PdfDocument, PdfPoints, PdfRenderConfig, PdfSearchOptions, Pdfium};

/// A rendered page.
pub struct Rendered {
    pub image: egui::ColorImage,
    /// Where the search words are, as fractions of the image (0–1), top left origin.
    pub hits: Vec<egui::Rect>,
    /// Pages in the document.
    pub pages: usize,
}

/// One page to render. `key` comes back with the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub key: String,
    pub path: PathBuf,
    pub page: usize,
    pub width: u32,
    /// Words to find on the page.
    pub terms: Vec<String>,
}

pub struct Renderer {
    tx: Sender<Request>,
    rx: Receiver<(String, Result<Rendered, String>)>,
}

impl Renderer {
    pub fn new(ctx: &egui::Context, pdfium_dir: Option<PathBuf>) -> Renderer {
        let (tx, jobs) = channel::<Request>();
        let (out, rx) = channel();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("vf-pages".into())
            .spawn(move || {
                let pdfium = pdfium_dir
                    .ok_or_else(|| "pdfium.dll not found".to_string())
                    .and_then(|d| filer_pdf::pdf::pdfium(&d));
                for req in jobs {
                    let r = match &pdfium {
                        Ok(p) => render(p, &req.path, req.page, req.width, &req.terms),
                        Err(e) => Err(e.clone()),
                    };
                    if out.send((req.key, r)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn page renderer");
        Renderer { tx, rx }
    }

    pub fn request(&self, req: Request) {
        let _ = self.tx.send(req);
    }

    pub fn finished(&self) -> Vec<(String, Result<Rendered, String>)> {
        self.rx.try_iter().collect()
    }
}

/// Renders page `page` (0-based) of `path`, `width` pixels wide, and finds `terms` on it.
pub fn render(
    pdfium: &Pdfium,
    path: &Path,
    page: usize,
    width: u32,
    terms: &[String],
) -> Result<Rendered, String> {
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| format!("cannot open the PDF: {e}"))?;
    render_doc(&doc, page, width, terms)
}

fn render_doc(
    doc: &PdfDocument<'_>,
    page: usize,
    width: u32,
    terms: &[String],
) -> Result<Rendered, String> {
    let pages = doc.pages().len() as usize;
    let index = i32::try_from(page.min(pages.saturating_sub(1))).map_err(|e| e.to_string())?;
    let p = doc
        .pages()
        .get(index)
        .map_err(|e| format!("no page {}: {e}", page + 1))?;
    let config = PdfRenderConfig::new().set_target_width(width as i32);
    let img = p
        .render_with_config(&config)
        .and_then(|b| b.as_image())
        .map_err(|e| format!("cannot render page {}: {e}", page + 1))?
        .to_rgba8();
    let (w, h) = (img.width() as f32, img.height() as f32);
    let mut hits = Vec::new();
    if let Ok(text) = p.text() {
        for term in terms.iter().filter(|t| !t.trim().is_empty()) {
            let Ok(search) = text.search(term, &PdfSearchOptions::new()) else {
                continue;
            };
            while let Some(found) = search.find_next() {
                for seg in found.iter() {
                    let b = seg.bounds();
                    let corner = |x: PdfPoints, y: PdfPoints| {
                        p.points_to_pixels(x, y, &config)
                            .map(|(px, py)| egui::pos2(px as f32 / w, py as f32 / h))
                    };
                    if let (Ok(a), Ok(c)) =
                        (corner(b.left(), b.top()), corner(b.right(), b.bottom()))
                    {
                        hits.push(egui::Rect::from_two_pos(a, c));
                    }
                }
                if hits.len() > 500 {
                    break;
                }
            }
        }
    }
    Ok(Rendered {
        image: egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.as_raw(),
        ),
        hits,
        pages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_words_on_the_rendered_page() {
        let t = tempfile::tempdir().unwrap();
        let pdf = t.path().join("a.pdf");
        filer_pdf::pdf::write_test_pdf(&pdf, &["Cover", "Payer EIN 12-3456789"]);
        let dir = filer_pdf::pdf::find_pdfium(Path::new("")).unwrap();
        let p = filer_pdf::pdf::pdfium(&dir).unwrap();
        let r = render(p, &pdf, 1, 400, &["3456789".into()]).unwrap();
        assert_eq!(r.pages, 2);
        assert_eq!(r.image.size[0], 400);
        assert_eq!(r.hits.len(), 1);
        let h = r.hits[0];
        assert!(
            h.min.x > 0.0 && h.max.x <= 1.0 && h.min.y >= 0.0 && h.max.y <= 1.0,
            "{h:?}"
        );
        assert!(h.width() > 0.0 && h.height() > 0.0);
        assert!(
            render(p, &pdf, 0, 400, &["3456789".into()])
                .unwrap()
                .hits
                .is_empty()
        );
    }
}
