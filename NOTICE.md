# Vibe Files — notice

Copyright 2025-2026 Kisaes LLC, Monett, Missouri, USA

Vibe Files is published by Kisaes LLC. It is made of parts under different
licences.

## 1. Vibe Files (`vibe-files.exe`, `vibe-move.exe`, `vibe-scan32.exe`)

Licensed under the **VibeMyFirm Community License 1.0** (`LICENSE.md`,
installed as `LICENSE.txt`; https://vibemyfirm.com/licenses/community-1.0),
for version 0.1.52 and every later release (owner decision 2026-11-22).
Vibe Files is "other software Kisaes distributes under these terms" in the
licence's first lines.

In short (not part of the licence): use it free for the documents and records
of entities you own, control or work for; keeping, filing or working on the
documents and records of clients who pay you is Professional Use and needs a
Pro License Key; distributing it, changed or not, needs a Developer License
Key. Read `LICENSE.md` for the terms.

Until Pro keys: Kisaes LLC does not sell or issue Pro License Keys for Vibe
Files yet; they come with a later version. Until Kisaes announces them on the
releases page, Kisaes permits Professional Use of the 0.x test versions
without a key, and the app checks no key.

The engine: Vibe Files contains no Kisaes GL Engine and no other accounting
engine, so section 8 of the licence (The Engine) has nothing to apply to.

Earlier terms: versions 0.1.51 and earlier were published under the GNU
Affero General Public License v3.0 or later. Copies of those versions already
obtained keep those terms. Nothing from 0.1.52 on is under them.

Required Notice: Copyright © 2026 Kisaes LLC. Licensed under the VibeMyFirm
Community License 1.0, https://vibemyfirm.com/licenses/community-1.0

## 2. Code from other Kisaes LLC products

The folder tree, client sentinel, filing rules, Intake tab and document-moving
engine (`filer-core`, `filer-pdf`, `filer-intake`) come from Vibe Filer
Desktop. Kisaes LLC owns that codebase. The code is part of this app and
licensed with it, under section 1.

## 3. Bundled programs and libraries

- **PDFium** (`pdfium.dll`): BSD-3-Clause and Apache-2.0, with the licences of
  its parts (`pdfium-LICENSE`, `pdfium-licenses\`).
- **TWAIN Data Source Manager** (`TWAINDSM.dll`): LGPL-2.1-or-later
  (`TWAINDSM-LICENSE.txt`). An unmodified library loaded at run time; you may
  replace it with another build. Its source is published by the TWAIN Working
  Group at https://github.com/twain/twain-dsm.
- **Tesseract OCR** (`tesseract\`): Apache-2.0 (`tesseract\LICENSE`), source
  at https://github.com/tesseract-ocr/tesseract.

## 4. Rust libraries

`THIRD-PARTY-NOTICES.txt`, shipped with the app and written by
`tools/third-party-notices.ps1`, lists every Rust crate compiled into the three
programs with its licence and licence files. For the crates that publish no
licence file, it carries the standard text of the licence they name.
