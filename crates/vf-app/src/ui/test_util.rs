//! UI test helpers shared by the tab tests.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

/// How many frames a test's `run` may take to settle. kittest's own limit is 4. A worker
/// thread (the document preview, a job) asks the window to repaint when it is done, at a
/// moment of its own, and under a full run that pushed a `run` to 5 or 6 frames now and then:
/// the test then failed with "exceeded max_steps" though nothing was wrong.
pub const MAX_STEPS: u64 = 40;
/// How long a test waits for work done on another thread (Filer's loading, a push, a count)
/// before it fails. Such a wait ends as soon as the work is done; the limit is only for work
/// that never comes. 30 seconds was not always enough on a machine busy with something else.
pub const PATIENCE: std::time::Duration = std::time::Duration::from_secs(180);

/// The processor time this thread has used so far (kernel and user). A speed test measures
/// the code with this, not with a clock: another program building on the same machine made
/// the clock say 110 and 159 ms for work that takes about 40, and the test failed for it.
pub fn thread_cpu() -> std::time::Duration {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let (mut made, mut ended, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the pseudo-handle of this thread, and four FILETIMEs that outlive the call.
    let ok = unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut made,
            &mut ended,
            &mut kernel,
            &mut user,
        )
    };
    assert!(ok.is_ok(), "thread times not read");
    let ticks = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    // In 100-nanosecond units.
    std::time::Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}

/// Clicks the button labelled `label`, then runs until the UI settles.
pub fn click<S>(h: &mut Harness<'_, S>, label: &str) {
    h.get_by_role_and_label(Role::Button, label).click();
    h.run();
}

/// Focuses the field labelled `label` and types `text` (appended to what is there).
pub fn type_into<S>(h: &mut Harness<'_, S>, label: &str, text: &str) {
    h.get_by_label(label).click();
    h.step();
    h.get_by_label(label).type_text(text);
    h.step();
}

/// Binds the process-wide pdfium before a test starts anything that looks for it. The vendored
/// `filer_pdf::pdf::find_pdfium` test-binds the library; if that overlaps the first
/// `filer_pdf::pdf::pdfium` call on another test thread, the "already initialized" error is
/// cached for the whole test process. The app binds once at startup, so only tests overlap.
pub fn pdfium_first() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Some(dir) = filer_pdf::pdf::find_pdfium(std::path::Path::new("")) {
            let _ = filer_pdf::pdf::pdfium(&dir);
        }
    });
}

/// A PDF whose pages carry these lines of text, one printed row each (as UltraTax prints its
/// reports), for tests that read text through pdfium. Invented content only.
pub fn write_lines_pdf(path: &std::path::Path, pages: &[&[&str]]) {
    use lopdf::content::{Content, Operation};
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
    let mut kids: Vec<Object> = Vec::new();
    for lines in pages {
        let mut ops = vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 9.into()]),
            Operation::new("TL", vec![12.into()]),
            Operation::new("Td", vec![36.into(), 750.into()]),
        ];
        for l in *lines {
            ops.push(Operation::new("Tj", vec![Object::string_literal(*l)]));
            ops.push(Operation::new("T*", vec![]));
        }
        ops.push(Operation::new("ET", vec![]));
        let content_id = doc.add_object(Stream::new(
            dictionary! {},
            Content { operations: ops }.encode().unwrap(),
        ));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        kids.push(page_id.into());
    }
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

/// An invented UltraTax-style return (names and numbers made up).
pub const SAMPLE_RETURN: &[&[&str]] = &[
    &[
        "Filing Instructions",
        "Taxable Year Ended December 31, 2025",
    ],
    &[
        "Form Salaries & Wages Report",
        "T/S Employer Federal Wages Federal Withheld",
        "T SAMPLE ACCOUNTING, P.C. 21,500 2,100",
        "S EXAMPLE R-I SCHOOL DISTRICT",
        "Pg 12",
        "Avery & Morgan Sample",
        "Name",
    ],
    &[
        "1040 Pension/Annuity Report 2025",
        "T/S Payer Gross Distribution",
        "T STATE TEACHERS RETIREMENT SYS 7 18,000 18,000",
    ],
    &[
        "***-**-0000 Federal Statements Page 1",
        "Form 1040, Dividend Income",
        "Payer",
        "SAMPLE FUNDS (4321)",
        "$ 120 $ 80 $ 5",
        " TOTAL",
        "Schedule A, Line 8a - Home Mortgage Interest & Points From Form 1098",
        "Description Amount",
        "SAMPLE MORTGAGE LOAN $ 8,100",
        " TOTAL $ 8,100",
        "Statement 1 - Schedule E, Page 2, Line 28",
        "EXAMPLE HOLDINGS, LLC",
        "P **-***0002 6,000",
        " TOTAL $ 6,000",
        "Form 1040, Line 26 - Estimated Tax Payments and Amount Applied From Previous Year",
        "1ST QUARTER ESTIMATE PAYMENT 4,000",
        " TOTAL $ 4,000",
    ],
];
