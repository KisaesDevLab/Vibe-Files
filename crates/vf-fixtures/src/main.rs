//! `cargo run -p vf-fixtures [-- <out> [seed]]` → `fixtures/cabinet/` by default.

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cabinet")
    });
    let seed = args.next().map(|s| s.parse()).transpose()?.unwrap_or(42);
    let mut s = vf_fixtures::generate(&out, seed)?;
    // Scanned (image-only) documents for OCR (P03).
    let exe_dir = std::env::current_exe()?
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let dir = filer_pdf::pdf::find_pdfium(&exe_dir)
        .ok_or_else(|| anyhow::anyhow!("pdfium.dll not found (needed to draw the scans)"))?;
    let pdfium = filer_pdf::pdf::pdfium(&dir).map_err(anyhow::Error::msg)?;
    vf_fixtures::add_scans(&out, &mut s, pdfium, seed)?;
    println!(
        "{} clients, {} documents → {}",
        s.clients,
        s.docs.len(),
        out.display()
    );
    Ok(())
}
