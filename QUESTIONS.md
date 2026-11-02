# Vibe Files — QUESTIONS.md

Open questions, each with a recorded default. If a phase hits a question with a default, use the default, note the assumption in STATE.md, and keep going. If there is no default, stop and ask Kurt.

Decisions already locked on 2026-11-01 are at the bottom for reference; do not re-ask them.

---

## Open

**Q-01 — ~~Practice Manager bucket layout document.~~** Closed 2026-11-02: ignore Practice Manager Filer; use Filer Desktop. Vibe Files writes its own `docs/BUCKET_LAYOUT.md`.

**Q-02 — Scanner API: TWAIN or WIA?**
Default: one-day spike each in P02; choose WIA if it drives Kurt's office scanner with duplex + ADF; else TWAIN. Record result here.
**Deferred 2026-11-02 (owner):** the spike needs the office scanner attached, so P02 step 2 runs in a Claude Code session on Kurt's Windows PC. Until then, scans enter through `<data>/Scans` (scan-to-folder from the copier or the scanner's own software); cover sheets, splitting and routing already work on that folder.

**Q-03 — ~~Practice Manager decryption scheme.~~** Closed 2026-11-02: no PM dependency; only Vibe Files devices decrypt. Revisit under PA-11 if ever needed.

**Q-04 — Firm key custody for v2.** v1 uses one firm key held only by the firm (DPAPI + recovery passphrase + admin export). For sellable v2: per-client keys?
Default: leave as single firm key; add to PA-10 scope.

**Q-05 — Retention schedule defaults.** Seed the opinionated schedule in P07 (7y returns/workpapers, 4y source docs, etc.) or ship blank?
Default: seed it, editable.

**Q-06 — Windows auto-login mapping.** Map Windows SID → user automatically on first login from a new SID (self-enroll) or require admin pre-mapping?
Default: admin pre-mapping; self-enroll only when the cabinet has zero users (first run).

**Q-07 — OCR language packs.** English only?
Default: English only in v1; `eng.traineddata` bundled.

**Q-08 — TIN storage.** Store recipient TIN last-4 only (P08), never the full value, even encrypted?
Default: last-4 only. Full TIN never persisted outside the PDF itself.

**Q-09 — PDF-printer capture folder parsers.** Which tax software filename conventions are in scope for v1 parser profiles?
Default: UltraTax CS first (Kurt's primary), Lacerte and CCH Axcess second, Drake/ProSeries as untested stubs.

**Q-10 — Object Lock default.** Enable B2 Object Lock (immutability window) by default on new cabinets?
Default: off by default, strongly suggested in the Sync setup screen with a 30-day window.

**Q-11 — ~~Practice Manager read-side parity.~~** Closed 2026-11-02: Vibe Files generates and syncs thumbnails under `_thumbs/` in its own layout (P12 step 6).

**Q-12 — Remote-mode cache cap and eviction.** 5 GB default LRU?
Default: 5 GB, configurable.

**Q-13 — Audit retention.** Keep audit rows forever, or roll up after N years?
Default: forever; JSONL monthly files make it cheap.

**Q-14 — Conflict-copy naming.** `name (conflict from <device-name>).ext` — use device display name or device id?
Default: display name, fall back to id-short.

**Q-15 — Soft-delete trash expiry.** Auto-empty trash after N days when no retention/hold applies?
Default: no auto-empty in v1; manual, admin-gated.

**Q-16 — Splitting the single-crate Filer Desktop.** Upstream (0.2.4) has no `lib.rs`; the three `filer-*` crates don't exist.
Default: mechanical split per PHASES P00 step 2, no logic edits; merge crates rather than edit code if modules are circular. Alternative if the split is messy: vendor one `filer` crate with a `lib.rs` and treat the three names as modules. Record which one was done here.
**Result (P00 step 2, 2026-11-02):** three-crate split done with no logic edits. `sentinel.rs` went to `filer-intake` (it calls `commit`); four `#[cfg(test)]` helpers became `cfg(any(test, feature = "test-support"))`. Details in `crates/filer-*/VENDORED.md`.

**Q-17 — Where Overwrite keeps the old file.** Filer moves it to `data/overwritten/<batch>/` and prunes it when the next batch commits (undo is last-batch only).
Default: in Vibe Files, the wrapper also stores the overwritten content as a version in `_Vibe/versions/` (dedupe by hash) so nothing is lost after the next batch; Filer's own `data/overwritten` behavior stays as-is for its undo.

**Q-18 — Tracking upstream Filer Desktop.** Upstream is still in 0.2.x pre-release.
Default: take the newest `master` at P00 step 2; later upstream fixes come in only as a PA-xx re-vendor step with the test suite re-run. Never edit vendored files to "catch up".

**Q-19 — Tying Filer's Confirm to a Vibe Files Transaction.** *Answered 2026-11-02 (owner): a small vendored hook.* `filer-intake` gets `Hooks { commit, unpack, undo }` (VENDORED.md change 5) defaulting to Filer's own functions; Vibe Files wraps them so a batch lands on disk and in the catalog together, or Filer's undo puts it back.

**Q-20 — HEIC decoding.** *Answered 2026-11-02 (owner): bundle libheif.* vcpkg `libheif[core,libde265]` as DLLs (LGPL-3.0, dynamically linked) beside the exe, built in CI and release. Never the port's default `hevc` feature: it is the GPL-2.0 x265 encoder. Dev boxes without libheif build without feature `heic` and refuse HEIC files.

---

## Locked decisions (2026-11-01 Q&A) — do not re-ask

- Name: **Vibe Files**. New repo; Filer Desktop crates vendored in; Filer becomes the Intake tab of one exe.
- (2026-11-02) Ignore Practice Manager Filer; Filer Desktop is the only upstream (sentinel, folder tree, moves).
- (2026-11-02) Moving of documents is based on the most recent Vibe Filer Desktop repo (`KisaesDevLab/Vibe-Filer-Desktop`, `ea4e726`, v0.2.4 at the time).
- License: PolyForm Internal Use. v1 internal to Kisaes; sellable v2 after one tax season.
- Tiers in v1: single PC **and** LAN share. Appliance tier is v2+.
- Sync: two-way via B2 on Vibe Files' own layout (mirrors the Filer Desktop tree); serves offsite backup and remote mode. Last v1 phase (P11), schema sync-aware from P00. *(Amended 2026-11-02: Practice Manager layout and portal dropped.)*
- Conflicts: check-out lock prevents; unlocked collision → conflict copy, never overwrite.
- Encryption: client-side, firm cabinet key held by the firm only. *(Amended 2026-11-02: no PM server copy.)*
- Remote mode: catalog + thumbnails local, bytes on demand into LRU cache, pin client; metadata-only search in v1.
- Print-to-DMS: watch an existing PDF printer's output folder; no driver bundling.
- OCR: engine trait, Tesseract default, ONNX slot for v2.
- Extraction: v1, local patterns only (no AI Router).
- PDF v1: viewer, annotations/tickmarks, stamps, flatten, PDF/A, redaction with SSN/EIN auto-find. Bates/compare/form-fill → v2.
- Compliance v1: audit chain, retention + disposition + legal hold, §7216 consents, Safeguards appendix.
- Users: local users + roles + Windows auto-login. Vibe Auth → v2.
- Intake v1: Filer's sources + TWAIN/WIA scanner + barcode sheets + image/HEIC normalization. No email ingestion.
- Office files: store/open only. No workflow (PM owns it). Importer: generic tree wizard only.
