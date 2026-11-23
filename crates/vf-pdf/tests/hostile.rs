//! A crafted PDF must not crash the app (QUESTIONS Q-146, RUSTSEC-2026-0187): lopdf before 0.42
//! parsed nested arrays with unbounded recursion, so a ~21 KB file with ~10,000 nested arrays in
//! its Catalog overflowed the stack and aborted the process (not a panic: nothing could catch it).
//! Run on a test thread's default 2 MiB stack, as the app's worker threads are.

use lopdf::Document;

/// A small, otherwise valid PDF whose Catalog holds `depth` nested arrays.
fn nested_pdf(depth: usize) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    offsets.push(out.len());
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /X ");
    out.extend(std::iter::repeat_n(b'[', depth));
    out.extend(std::iter::repeat_n(b']', depth));
    out.extend_from_slice(b" >>\nendobj\n");
    offsets.push(out.len());
    out.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[test]
fn a_deeply_nested_pdf_is_refused_or_read_without_crashing() {
    let bytes = nested_pdf(10_400);
    assert!(bytes.len() > 20_000);
    // Either answer is fine; returning at all is the test.
    let _ = Document::load_mem(&bytes);
}

#[test]
fn a_shallow_nested_pdf_still_reads() {
    let d = Document::load_mem(&nested_pdf(20)).expect("a normal file still opens");
    assert!(d.catalog().is_ok());
}
