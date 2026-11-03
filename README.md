# Vibe Files

A Windows desktop document management system for small CPA and tax firms, written in Rust
([egui](https://github.com/emilk/egui)). Vibe Files is a cabinet layer over a plain client folder
tree. It adds browsing, intake, OCR, full-text search, PDF tools, versions, an audit trail,
retention, and two-way sync to Backblaze B2.

It works on the client folder tree, sentinels and document-moving engine of
[Vibe Filer Desktop](https://github.com/KisaesDevLab/Vibe-Filer-Desktop). Filer's engine is
vendored here, and Filer itself runs as the Intake tab.

> **Status: in development, not released.** Built phase by phase, see [PHASES.md](PHASES.md).
> [STATE.md](STATE.md) says what is done and what comes next. v1 is for internal use at
> Kisaes LLC.

## Principles

- **Files stay plain files.** Every PDF or Office file in the cabinet opens in Explorer without
  Vibe Files. Vibe Files never uses a blob store or a proprietary container.
- **The catalog is a rebuildable index.** It is a SQLite database beside the exe.
  `vibe-files rebuild` recreates it from the folder tree, the client sentinels and the
  `_Vibe/` sidecar files.
- **Offline first.** Every v1 feature works with no network. B2 sync is additive.
- **Portable.** It ships as `vibe-files.exe` plus a `data/` folder beside it. There is no
  installer and no registry writes, except an optional Explorer menu entry.
- **Every change is a transaction.** A change writes the file, the catalog row, the audit row and
  the undo record together, or none of them.
- **No telemetry.** The only network calls go to the firm's own B2 bucket.

## What works today

| Area | |
|---|---|
| Cabinet | Create or open a cabinet over an existing Filer client tree, with nothing to type but the folder. |
| Browse | Client, year and folder tree, views (Recent, Pinned, Needs doc type, …), metadata editing, bulk edit, rename, undo. |
| Clients | Filer client master, CSV import, relationships, doc types and tax years. |
| Intake | The Filer Desktop app mounted unchanged: rules, preview and confirm, collisions, undo of the last batch. Every batch is recorded in the catalog. |
| Scanning | WIA scanner profiles (feeder, duplex, dpi, color, blank-page drop, deskew), barcode cover and separator sheets that split a scanned stack, photo and HEIC → PDF, and a print-capture folder with an UltraTax CS filename parser. |
| OCR | Tesseract (separate program) through a job queue that resumes after a restart. Scans get an invisible text layer written into the PDF, and the original is kept as a version. |
| Search | FTS5 full text, with partial matches, combined with metadata filters (client, year, doc type, status, source, dates, tag). Snippets, and results open at the matching page. |

Planned next: the PDF viewer and annotations, versions UI, retention and legal hold, form
extraction, an import wizard, B2 sync, and hardening.
[PHASES.md](PHASES.md) lists the deferred items and v2 candidates.

## Building

Requirements:

- Windows 10 or 11, x64
- Rust stable (2024 edition) with the MSVC toolchain
- For OCR: Tesseract 5 with English data. Install it with
  `winget install UB-Mannheim.TesseractOCR`, or put a `tesseract\` folder beside `vibe-files.exe`.

```powershell
cargo build --release -p vf-app             # target\release\vibe-files.exe
cargo test --workspace                      # the test suite (OCR tests need Tesseract)
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p vf-fixtures                    # synthetic test cabinet in fixtures\cabinet\
```

`pdfium.dll` ships in `crates/filer-pdf/assets/` and the 64-bit TWAIN manager
`TWAINDSM.dll` in `crates/vf-ingest/assets/`. Copy both beside the exe for a portable build
(TWAINDSM.dll is also found in Windows' System32 when a scanner driver installed it).

## Repository layout

```
crates/
  filer-core, filer-pdf, filer-intake   vendored Vibe Filer Desktop (see each VENDORED.md)
  vf-core      catalog schema, documents, Transaction, audit
  vf-fs        tree walker, rebuild, file operations through Filer's move engine
  vf-ingest    scanner, barcode sheets, image → PDF, print capture
  vf-ocr       OCR engine trait, Tesseract, job queue, text-layer write-back
  vf-index     full-text and metadata search
  vf-app       the egui application and the CLI
  vf-fixtures  synthetic cabinet generator (no real client data)
docs/          UI spec, bucket layout, sentinel format
```

Contributors start with [CLAUDE.md](CLAUDE.md), the standing project brief, then
[STATE.md](STATE.md), [PHASES.md](PHASES.md) and [QUESTIONS.md](QUESTIONS.md).

## Test data

Fixtures, tests and logs never contain real client data. The fixture generator uses only
synthetic names, SSNs in the never-issued 987-65-432x range, and EIN prefixes the IRS never
assigned.

## License

[GNU Affero General Public License v3.0 or later](LICENSE) (`AGPL-3.0-or-later`).
Third-party components keep their own licenses. pdfium's licenses are in
`crates/filer-pdf/assets/`. The TWAIN Data Source Manager (TWAIN Working Group, 2.5.1) is
LGPL-2.1-or-later, licence in `crates/vf-ingest/assets/`. Tesseract is installed separately
and is Apache-2.0.
