//! Real fillable forms (Q-61): a blank public form (IRS 1099-NEC, never a client document) is
//! filled with invented values, then read the way Intake reads it. The values typed into its
//! fields must reach the form reader.
//!
//! `VF_FORMS_DIR=<dir with f1099nec.pdf> cargo test -p vf-ingest --test forms_real -- --ignored --nocapture`

use std::path::Path;

#[test]
#[ignore = "needs VF_FORMS_DIR with the blank IRS f1099nec.pdf"]
fn filled_1099_nec_is_read() {
    let dir = std::env::var("VF_FORMS_DIR").expect("VF_FORMS_DIR");
    let src = Path::new(&dir).join("f1099nec.pdf");
    let t = tempfile::tempdir().unwrap();
    let filled = t.path().join("filled.pdf");
    let f = |n: &str| format!("topmostSubform[0].CopyB[0].{n}");
    // Invented: a never-issued SSN range and an EIN prefix the IRS never assigned.
    vf_pdf::forms::fill(
        &src,
        &filled,
        &[
            (f("PgHeader[0].CalendarYear[0].f2_1[0]"), "2025".into()),
            // Payer's name, street, city; the two TINs; recipient's name, street, city.
            (f("LeftCol[0].f2_2[0]"), "Invented Payer LLC".into()),
            (f("LeftCol[0].f2_3[0]"), "1 Sample Way".into()),
            (f("LeftCol[0].f2_5[0]"), "Sampletown".into()),
            (f("LeftCol[0].f2_10[0]"), "00-0000123".into()),
            (f("LeftCol[0].f2_11[0]"), "987-65-4321".into()),
            (f("LeftCol[0].f2_12[0]"), "Pat Q Sample".into()),
            (f("LeftCol[0].f2_13[0]"), "2 Example Rd".into()),
            (f("LeftCol[0].f2_15[0]"), "Sampletown".into()),
            (f("RightCol[0].f2_20[0]"), "1234.00".into()),
        ],
    )
    .unwrap();
    let dir = filer_pdf::pdf::find_pdfium(Path::new("")).expect("pdfium");
    let pdfium = filer_pdf::pdf::pdfium(&dir).unwrap();
    let read = vf_ingest::recognize::form_read(pdfium, &filled, None).expect("read");
    for (l, v) in &read.fields {
        println!("{l} = {v}");
    }
    let e = vf_extract::recognize_filled(&read.text, &read.fields).expect("recognized");
    println!("{e:?}");
    assert_eq!(e.form.key(), "1099-nec");
    assert_eq!(e.tax_year, Some(2025));
    assert_eq!(e.recipient_name.as_deref(), Some("Pat Q Sample"));
}
