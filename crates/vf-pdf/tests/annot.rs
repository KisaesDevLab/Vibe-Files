//! Annotations written by `vf_pdf::annot` read back with their appearance, author and Print
//! flag, and render where they were put (the Acrobat round-trip itself is checked by the
//! owner: STATE).

use std::path::Path;

use pdfium_render::prelude::{PdfRenderConfig, Pdfium};
use vf_pdf::annot::{self, Annotation, Kind, Tick};

fn pdfium() -> &'static Pdfium {
    let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium");
    filer_pdf::pdf::pdfium(&dir).unwrap()
}

/// Page 0 rendered 612 px wide (1 px per point), RGBA.
fn render(p: &Path, page: i32) -> (u32, u32, Vec<u8>) {
    let doc = pdfium().load_pdf_from_file(p, None).unwrap();
    let img = doc
        .pages()
        .get(page)
        .unwrap()
        .render_with_config(
            &PdfRenderConfig::new()
                .set_target_width(612)
                .render_annotations(true),
        )
        .unwrap()
        .as_image()
        .unwrap()
        .to_rgba8();
    (img.width(), img.height(), img.into_raw())
}

/// Pixels that differ inside a user-space rect of an upright Letter page.
fn changed_in(a: &[u8], b: &[u8], w: u32, h: u32, r: [f32; 4]) -> usize {
    let (x0, x1) = (r[0].max(0.0) as u32, (r[2] as u32).min(w - 1));
    let (y0, y1) = (
        (h as f32 - r[3]).max(0.0) as u32,
        ((h as f32 - r[1]) as u32).min(h - 1),
    );
    let mut n = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let i = ((y * w + x) * 4) as usize;
            if a[i..i + 3] != b[i..i + 3] {
                n += 1;
            }
        }
    }
    n
}

fn every_kind() -> Vec<Annotation> {
    let a = |kind| Annotation {
        page: 0,
        kind,
        color: annot::RED,
        author: "KWK".into(),
    };
    vec![
        Annotation {
            color: annot::YELLOW,
            ..a(Kind::Highlight(vec![[72.0, 700.0, 300.0, 716.0]]))
        },
        a(Kind::Underline(vec![[72.0, 650.0, 300.0, 664.0]])),
        a(Kind::StrikeOut(vec![[72.0, 600.0, 300.0, 614.0]])),
        a(Kind::Note {
            at: [400.0, 700.0],
            text: "Ask the client about box 12".into(),
        }),
        a(Kind::TextBox {
            rect: [350.0, 560.0, 560.0, 620.0],
            text: "Agrees to the 1099 from the bank".into(),
            size: 11.0,
        }),
        a(Kind::Ink(vec![vec![
            [80.0, 480.0],
            [120.0, 520.0],
            [160.0, 480.0],
            [200.0, 520.0],
        ]])),
        a(Kind::Arrow {
            from: [250.0, 450.0],
            to: [350.0, 500.0],
        }),
        a(Kind::Tickmark {
            tick: Tick::Check,
            at: [100.0, 380.0],
            size: 28.0,
        }),
        a(Kind::Tickmark {
            tick: Tick::PriorYear,
            at: [150.0, 380.0],
            size: 28.0,
        }),
        Annotation {
            color: annot::ACCENT,
            ..a(Kind::Stamp {
                text: "RECEIVED 10/29/2025".into(),
                at: [400.0, 300.0],
                size: 16.0,
            })
        },
    ]
}

#[test]
fn every_kind_reads_back_and_renders_in_its_box() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src.pdf");
    filer_pdf::pdf::write_test_pdf(&src, &["Page one", "Page two"]);
    let out = t.path().join("out.pdf");
    let annots = every_kind();
    assert_eq!(
        annot::add(&src, &out, &annots, "2026-11-03T14:05:09Z").unwrap(),
        annots.len()
    );

    let found = annot::read(&out).unwrap();
    assert_eq!(found.len(), annots.len());
    let subtypes: Vec<&str> = found.iter().map(|f| f.subtype.as_str()).collect();
    assert_eq!(
        subtypes,
        [
            "Highlight",
            "Underline",
            "StrikeOut",
            "Text",
            "FreeText",
            "Ink",
            "Line",
            "Stamp",
            "Stamp",
            "Stamp"
        ]
    );
    for f in &found {
        assert!(f.has_appearance, "{f:?}");
        assert!(f.printable, "{f:?}");
        assert_eq!(f.author.as_deref(), Some("KWK"));
        assert_eq!(f.page, 0);
    }
    assert_eq!(found[7].contents.as_deref(), Some("Tickmark: agreed, KWK"));
    assert_eq!(found[9].contents.as_deref(), Some("RECEIVED 10/29/2025"));

    // Each one draws inside its own box (pdfium renders the appearance streams).
    let (w, h, before) = render(&src, 0);
    let (_, _, after) = render(&out, 0);
    for f in &found {
        assert!(
            changed_in(&before, &after, w, h, f.rect) > 10,
            "{} draws nothing in {:?}",
            f.subtype,
            f.rect
        );
    }
    // Page 2 untouched.
    assert_eq!(render(&src, 1).2, render(&out, 1).2);
}

/// A stamp on a page shown turned (`/Rotate 90`) gets a box turned to match, so it reads
/// upright on screen.
#[test]
fn stamps_on_a_rotated_page_read_upright() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src.pdf");
    filer_pdf::pdf::write_test_pdf(&src, &["Turned"]);
    let mut d = lopdf::Document::load(&src).unwrap();
    let id = *d.get_pages().values().next().unwrap();
    d.get_dictionary_mut(id).unwrap().set("Rotate", 90);
    d.save(&src).unwrap();

    let out = t.path().join("out.pdf");
    let stamp = Annotation {
        page: 0,
        kind: Kind::Stamp {
            text: "CLIENT COPY".into(),
            at: [300.0, 400.0],
            size: 20.0,
        },
        color: annot::ACCENT,
        author: "KWK".into(),
    };
    annot::add(&src, &out, &[stamp], "2026-11-03T14:05:09Z").unwrap();
    let f = &annot::read(&out).unwrap()[0];
    let (w, h) = (f.rect[2] - f.rect[0], f.rect[3] - f.rect[1]);
    assert!(h > w * 2.0, "the user-space box is turned: {w} x {h}");

    // On screen it is wider than tall.
    let doc = pdfium().load_pdf_from_file(&out, None).unwrap();
    let page = doc.pages().first().unwrap();
    let cfg = PdfRenderConfig::new()
        .set_target_width(792)
        .render_annotations(true);
    let with = page
        .render_with_config(&cfg)
        .unwrap()
        .as_image()
        .unwrap()
        .to_rgba8();
    let without = page
        .render_with_config(
            &PdfRenderConfig::new()
                .set_target_width(792)
                .render_annotations(false),
        )
        .unwrap()
        .as_image()
        .unwrap()
        .to_rgba8();
    let (mut x0, mut x1, mut y0, mut y1) = (u32::MAX, 0, u32::MAX, 0);
    for (x, y, p) in with.enumerate_pixels() {
        if p.0 != without.get_pixel(x, y).0 {
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
    }
    assert!(
        x1 > x0 && (x1 - x0) > 2 * (y1 - y0),
        "{x0}..{x1} x {y0}..{y1}"
    );
}

/// Step 3: "Stamp all pages" at a placement preset.
#[test]
fn stamp_all_pages_top_right() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src.pdf");
    filer_pdf::pdf::write_test_pdf(&src, &["One", "Two", "Three"]);
    let out = t.path().join("out.pdf");
    let text = annot::Preset::Received.text("2026-10-29", "KWK");
    let spec = annot::StampSpec {
        text,
        size: 14.0,
        placement: annot::Placement::TopRight,
        pages: vec![],
        color: annot::ACCENT,
        author: "KWK".into(),
    };
    let n = annot::stamp_pages(&src, &out, &spec, "2026-10-29T09:00:00Z").unwrap();
    assert_eq!(n, 3);
    let found = annot::read(&out).unwrap();
    assert_eq!(found.iter().map(|f| f.page).collect::<Vec<_>>(), [0, 1, 2]);
    for f in &found {
        assert_eq!(f.contents.as_deref(), Some("RECEIVED 10/29/2026"));
        // Top right of a Letter page: right edge within the margin, top near the top.
        assert!(
            f.rect[2] <= 612.0 - 23.0 && f.rect[2] > 500.0,
            "{:?}",
            f.rect
        );
        assert!(
            f.rect[3] <= 792.0 - 23.0 && f.rect[3] > 700.0,
            "{:?}",
            f.rect
        );
    }
}

/// Step 4: flatten burns every annotation into the page; the page looks the same and has no
/// annotations left.
#[test]
fn flatten_burns_annotations_in() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src.pdf");
    filer_pdf::pdf::write_test_pdf(&src, &["Page one", "Page two"]);
    let annotated = t.path().join("annotated.pdf");
    annot::add(&src, &annotated, &every_kind(), "2026-11-03T14:05:09Z").unwrap();
    let flat = t.path().join("flat.pdf");
    let n = vf_pdf::flatten::flatten(&annotated, &flat).unwrap();
    assert_eq!(n, every_kind().len());
    assert!(
        annot::read(&flat).unwrap().is_empty(),
        "no annotations left"
    );

    let (w, h, with) = render(&annotated, 0);
    let (_, _, flattened) = render(&flat, 0);
    let differ = with
        .chunks(4)
        .zip(flattened.chunks(4))
        .filter(|(a, b)| {
            a.iter()
                .zip(b.iter())
                .take(3)
                .any(|(x, y)| x.abs_diff(*y) > 40)
        })
        .count();
    assert!(
        differ < (w * h) as usize / 200,
        "{differ} pixels differ after flattening"
    );
    // And it did draw something: the flattened page is not the bare page.
    let (_, _, bare) = render(&src, 0);
    assert_ne!(bare, flattened);
}

/// Step 6: rotate, delete and reorder keep each page's look and text.
#[test]
fn page_edits() {
    use vf_pdf::pages;
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src.pdf");
    filer_pdf::pdf::write_test_pdf(&src, &["Alpha", "Bravo", "Charlie", "Delta"]);
    let texts = |p: &Path| -> Vec<String> {
        let d = pdfium().load_pdf_from_file(p, None).unwrap();
        d.pages()
            .iter()
            .map(|pg| vf_pdf::text::page_text(&pg))
            .collect()
    };

    let r = t.path().join("rotated.pdf");
    pages::rotate(&src, &r, &[1], 90).unwrap();
    assert_eq!(texts(&r)[1].trim(), "Bravo", "a turned page keeps its text");
    let d = lopdf::Document::load(&r).unwrap();
    let ids: Vec<_> = d.get_pages().values().copied().collect();
    assert_eq!(vf_pdf::layer::page_rotate(&d, ids[1]), 90);
    assert_eq!(vf_pdf::layer::page_rotate(&d, ids[0]), 0);

    let o = t.path().join("reordered.pdf");
    pages::reorder(&r, &o, &[3, 0, 2, 1]).unwrap();
    let got = texts(&o);
    let first_words: Vec<&str> = got.iter().map(|t| t.trim()).collect();
    assert_eq!(first_words, ["Delta", "Alpha", "Charlie", "Bravo"]);
    let d = lopdf::Document::load(&o).unwrap();
    let ids: Vec<_> = d.get_pages().values().copied().collect();
    assert_eq!(
        vf_pdf::layer::page_rotate(&d, ids[3]),
        90,
        "Bravo kept its turn"
    );

    let x = t.path().join("deleted.pdf");
    pages::delete(&o, &x, &[0, 2]).unwrap();
    assert_eq!(
        texts(&x)
            .iter()
            .map(|t| t.trim().to_string())
            .collect::<Vec<_>>(),
        ["Alpha", "Bravo"]
    );
    assert!(pages::delete(&x, &t.path().join("none.pdf"), &[0, 1]).is_err());
    assert!(pages::reorder(&x, &t.path().join("bad.pdf"), &[0, 0]).is_err());
}

/// A letter tickmark (A–Z) in a chosen color is a real annotation that renders in that color
/// (owner 2026-11-04).
#[test]
fn letter_tickmarks_in_a_chosen_color() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src.pdf");
    filer_pdf::pdf::write_test_pdf(&src, &["Page one"]);
    let out = t.path().join("out.pdf");
    let red = annot::TICK_COLORS[1].1;
    let letters: Vec<Annotation> = [(0u8, 100.0), (25, 300.0)]
        .into_iter()
        .map(|(i, x)| Annotation {
            page: 0,
            kind: Kind::Tickmark {
                tick: Tick::Letter(i),
                at: [x, 500.0],
                size: 40.0,
            },
            color: red,
            author: "KWK".into(),
        })
        .collect();
    annot::add(&src, &out, &letters, "2026-11-04T10:00:00Z").unwrap();
    let found = annot::read(&out).unwrap();
    let contents: Vec<_> = found.iter().map(|f| f.contents.as_deref()).collect();
    assert_eq!(
        contents,
        [Some("Tickmark: A, KWK"), Some("Tickmark: Z, KWK")]
    );
    assert_eq!(Tick::letters().count(), 26);
    // Red ink: some pixel in the box is strongly red.
    let (w, _h, img) = render(&out, 0);
    let r = found[0].rect;
    let red_px = (r[0] as u32..r[2] as u32)
        .flat_map(|x| ((792.0 - r[3]) as u32..(792.0 - r[1]) as u32).map(move |y| (x, y)))
        .filter(|&(x, y)| {
            let i = ((y * w + x) * 4) as usize;
            img[i] > 150 && img[i + 1] < 80 && img[i + 2] < 80
        })
        .count();
    assert!(red_px > 10, "{red_px} red pixels");
}

/// A pasted tape (PA-14) is a real FreeText annotation that draws in its box, and its data
/// reads back so the tape can be restored.
#[test]
fn pasted_tape_is_a_real_annotation_and_reads_back() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src.pdf");
    filer_pdf::pdf::write_test_pdf(&src, &["Page one"]);
    let out = t.path().join("out.pdf");
    let row = |label: &str, amount: &str, mark: &str, total: bool| annot::TapeRow {
        label: label.into(),
        amount: amount.into(),
        mark: mark.into(),
        total,
    };
    let tape = Annotation {
        page: 0,
        kind: Kind::Tape {
            at: [300.0, 400.0],
            title: "1099-INT tie-out".into(),
            rows: vec![
                row("Interest income · p.1 box 1", "1,284.17", "+", false),
                row("p.1 box 3", "45.00", "+", false),
                row("", "1,329.17", "T", true),
            ],
            footer: "KWK · 11/04/2026 · ✓ ties to Sch B".into(),
            labels: true,
            data: r#"{"lines":[1,2]}"#.into(),
        },
        color: annot::ACCENT,
        author: "KWK".into(),
    };
    annot::add(
        &src,
        &out,
        std::slice::from_ref(&tape),
        "2026-11-04T10:00:00Z",
    )
    .unwrap();
    let found = annot::read(&out).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].subtype, "FreeText");
    assert!(found[0].has_appearance && found[0].printable);
    assert!(found[0].contents.as_deref().unwrap().contains("1,329.17 T"));
    let tapes = annot::read_tapes(&out).unwrap();
    assert_eq!(tapes.len(), 1);
    assert_eq!(tapes[0].title, "1099-INT tie-out");
    assert_eq!(tapes[0].data, r#"{"lines":[1,2]}"#);
    // It draws in its box.
    let (w, h, before) = render(&src, 0);
    let (_, _, after) = render(&out, 0);
    assert!(changed_in(&before, &after, w, h, found[0].rect) > 100);
}
