//! A filled form looks filled where pdfium draws it (the Viewer's pages, Print), not only in
//! the field's value (PA-27).

use std::path::Path;

use pdfium_render::prelude::{PdfRenderConfig, Pdfium};

fn pdfium() -> &'static Pdfium {
    let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium");
    filer_pdf::pdf::pdfium(&dir).unwrap()
}

/// Dark pixels inside user-space `rect` of page 1, rendered 612 px wide (1 px = 1 pt).
fn ink_in(path: &Path, rect: [f32; 4]) -> usize {
    let doc = pdfium().load_pdf_from_file(path, None).unwrap();
    let page = doc.pages().get(0).unwrap();
    let img = page
        .render_with_config(&PdfRenderConfig::new().set_target_width(612))
        .unwrap()
        .as_image()
        .unwrap()
        .to_luma8();
    let h = img.height() as f32;
    let mut n = 0;
    for y in (h - rect[3]) as u32..(h - rect[1]) as u32 {
        for x in rect[0] as u32..rect[2] as u32 {
            if img.get_pixel(x, y).0[0] < 128 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn filled_text_and_check_box_are_drawn() {
    let t = tempfile::tempdir().unwrap();
    let (src, out) = (t.path().join("f.pdf"), t.path().join("o.pdf"));
    vf_pdf::forms::write_test_form(&src);
    let name = [72.0, 700.0, 300.0, 720.0];
    let check = [72.0, 520.0, 84.0, 532.0];
    assert_eq!(ink_in(&src, name), 0, "empty before");
    assert_eq!(ink_in(&src, check), 0);
    vf_pdf::forms::fill(
        &src,
        &out,
        &[
            ("client_name".into(), "Sample Client Name".into()),
            ("signed".into(), "Yes".into()),
        ],
    )
    .unwrap();
    assert!(ink_in(&out, name) > 50, "the typed name is drawn");
    assert!(ink_in(&out, check) > 50, "the check box is drawn on");
}
