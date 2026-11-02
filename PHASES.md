# Vibe Files — PHASES.md

Each phase has Goal, Steps, Done when. Work them in order. A phase is complete only when every "Done when" item is checked and STATE.md says so. Addenda go at the end as PA-xx.

---

## P00 — Workspace, vendored Filer crates, sync-aware catalog schema

**Goal:** A compiling workspace with Filer Desktop's engine vendored in and a catalog schema that will not need rewriting for LAN or B2.

**Steps**
1. Create workspace per CLAUDE.md §Workspace. `vf-app` opens an empty egui window with the six tabs stubbed.
2. Vendor from the most recent Vibe Filer Desktop repo (`KisaesDevLab/Vibe-Filer-Desktop` @ `master` `ea4e7267797ee869f05c3e5745700e13910a2e5b` (v0.2.4, 2026-11-02); re-check `master` first and take a newer HEAD if there is one). Upstream is a **single binary crate with no `lib.rs`** (owner decision there, 2026-11-01), so there are no `filer-*` crates to copy. Do a mechanical split instead:
   - `filer-core`: `config`, `clients`, `sentinel`, `template`, `relationships`, `rules`, `rulebuild`, `parse` + `assets/default_template.json`.
   - `filer-pdf`: `pdf`, `split` (+ pdfium binding lookup).
   - `filer-intake`: `intake`, `zipimport`, `plan`, `commit`, `undo`, `oplog`, `recovery`, `worker`, `ui/*`, plus the parts of `app.rs` the Intake tab needs.
   - Only changes allowed: add `lib.rs` per crate, change `crate::x` paths to the new crate names, make items `pub` where another crate needs them, and point `env!("CARGO_MANIFEST_DIR")` fixture paths at the moved `tests/fixtures/`. No logic edits. Every upstream unit test (194 at 0.2.4) moves with its module and passes.
   - `crates/filer-*/VENDORED.md`: source repo, commit SHA, date, and a file → crate table. Keep upstream's `CLAUDE.md` §7 safety rules verbatim in `filer-intake/VENDORED.md`.
   - If a module can't be placed without a logic edit (circular dependency between `plan`/`commit` and `ui/preview`), merge the two crates involved rather than editing code, and record it in QUESTIONS Q-16.
3. Write `docs/SENTINEL_V1.md` from Filer Desktop's `src/sentinel.rs` at the vendored commit (fields `version`, `client_id`, `firm_id`, `tax_software_id`, `display_name_at_creation`, `created_at`, `created_by`; write-once; unparseable/duplicate/orphan handling; set-aside naming) and `docs/BUCKET_LAYOUT.md` as Vibe Files' own layout: `cabinet.json`, encrypted manifest (path ↔ SHA-256 ↔ `client_id`), content-addressed objects, `_thumbs/`, lock objects, tombstones, `layout_ver = 1`. Client paths mirror the Filer Desktop template tree. Do not consult Practice Manager.
4. `vf-core` migration 001:
   - `cabinet(id, firm_id, root_path, schema_ver, layout_ver, created)`
   - `client(id, ext_id, name, entity_type, folder_rel, sentinel_hash, …)` + `client_relationship(parent_id, child_id, kind, share_pct)`
   - `doc_type(id, name, default_folder_tpl, retention_class_id, client_visible, sort)`
   - `document(id uuid7, client_id, doc_type_id, tax_year nullable, rel_path, hash, size, mime, source, status, filed_by, filed_at, device_id, rev, deleted_at nullable, tombstone_reason nullable)`
   - `version(doc_id, hash, size, created, created_by, device_id, reason)`
   - `lock(doc_id, user, device_id, since, lease_until, scope)`
   - `audit(seq, ts, user, device_id, action, doc_id nullable, detail json, prev_hash, hash)`
   - `retention_class(id, name, years, anchor enum{tax_year_end, engagement_close, filed_at, never}, legal_hold_default)`
   - `legal_hold(id, client_id nullable, doc_id nullable, reason, since, released)`
   - `consent_7216(id, client_id, scope, signed_at, expires_at, doc_id)`
   - `user(id, username, display, role, windows_sid nullable, pw_hash nullable, totp nullable, active)` + `user_client_acl`
   - `saved_search(id, name, query json, owner)`
   - `sync_state(device_id, last_manifest_rev, last_push, last_pull)` and `sync_pending(doc_id, op, since)`
   - `device(id, name, first_seen, last_seen, mode enum{local, lan, remote})`
5. `vf-core::tx::Transaction` — builder that takes file ops + catalog ops + audit detail + undo record and commits atomically. File ops are executed by the vendored `filer-intake::commit` primitives (`transfer`, `copy_verified`, `move_no_replace`, `free_path`) — their temp-file + SHA-256 + rename behavior is the staging; catalog in one SQLite transaction; on catalog failure, the file ops are reversed with `filer-intake::undo` logic. Filer's `OpRecord`s map 1:1 to audit rows. `vf-core` stays free of UI/network deps, so the trait for file ops lives in `vf-core` and the impl that calls `filer-intake` lives in `vf-fs`.
6. Audit hash chain: write, verify, and a `vibe-files audit verify` CLI subcommand.
7. `vibe-files rebuild <cabinet>` walks the tree, reads sentinels, hashes files, and rebuilds `document`/`client`/`version` rows from disk. Idempotent.
8. Fixture cabinet generator: `cargo run -p vf-fixtures` builds `fixtures/cabinet/` with 5 synthetic clients, 2 years, ~60 synthetic PDFs (generated with `pdf-writer`, random names/EINs that fail checksum).

**Done when**
- [x] `cargo test --workspace` and clippy clean
- [x] Vendored crates build with only the mechanical changes listed in step 2, every upstream test passes (193/194 under Wine; `twenty_file_batch_resolves_to_ready` to confirm on Windows), and `VENDORED.md` names the exact Filer commit — *confirmed 2026-11-02: all 194 pass on `windows-latest` (CI run 41506666554)*
- [x] Commit → undo → commit through `Transaction` leaves the fixture cabinet and inbox byte-identical, for same-volume rename and for forced copy-verify (Filer's own round-trip test, run through the wrapper)
- [x] Migration 001 applies to an empty SQLCipher DB; `rebuild` on the fixture cabinet produces the expected row counts
- [x] Audit chain verify passes after 1,000 synthetic rows and fails after one is tampered
- [x] `docs/SENTINEL_V1.md` matches Filer Desktop's `Sentinel` struct (a test round-trips a sentinel written by vendored `filer-intake` through the doc's example JSON) and `docs/BUCKET_LAYOUT.md` is present

---

## P01 — Cabinet browse, client master, doc types, tax-year metadata

**Goal:** Open a cabinet and navigate it like a filing cabinet with CPA-aware metadata.

**Steps**
1. Cabinet open/create wizard: pick root path, name, set recovery passphrase, create `_Vibe/cabinet.json`, run rebuild.
2. Browse tab: left tree (Cabinet → Clients → Permanent / Years → folders), right list (name, doc type, year, status, size, modified, filed by, hash-short), bottom metadata panel. Virtual column sort, multi-select, keyboard navigation.
3. Client master: reuse `filer-core` CSV import + folder discovery; add entity type, relationships editor (entity ↔ shareholder/partner/beneficiary with share %). New-client wizard creates folder from template and writes sentinel.
4. Doc type catalog editor (Admin tab): seeded with ~40 types (W-2, 1099-INT/DIV/B/R/NEC/MISC/K, 1098, 1095, K-1 (1065/1120S/1041), SSA-1099, organizer, engagement letter, 8879, return copy — federal/state, extension, estimate vouchers, notice/correspondence, workpaper, trial balance, bank statement, depreciation schedule, formation docs, EIN letter, S election, operating agreement, POA 2848, 8821, consent 7216, invoice, misc).
5. Metadata panel edit: doc type, tax year, status (received / in work / final / delivered), source (client / firm / government / third party), tags. Bulk edit on multi-select via Transaction.
6. Bulk rename using Filer's token templates with preview.
7. Virtual folders: "Recent", "Pinned", "Needs doc type", "Needs tax year", "Received this week", plus per-user favorites.
8. Open-with: double-click opens in the registered app; "Open folder in Explorer"; copy path.

**Done when**
- [x] Fixture cabinet browses with <100 ms tree expansion at 5,000 documents — *`vf-core` `browse::tree_expansion_is_fast_at_5000_docs` (catalog rows, 50 clients)*
- [x] New client wizard creates a folder tree matching the template and a valid SentinelV1 — *`vf-fs` `clients::new_client_matches_template_and_sentinel_reads_back`*
- [x] Bulk edit of 200 docs is one Transaction, one audit batch, one undo — *`vf-core` `meta::bulk_edit_200_docs_is_one_transaction`*
- [x] Relationships round-trip through `filer-core`'s existing format — *`vf-fs` `clients::relationships_round_trip_through_filer_format`*

---

## P02 — Intake: Filer tab, scanner, barcode sheets, image normalization, PDF-printer watcher

**Goal:** Every way paper and PDFs arrive lands in one preview/confirm queue.

**Steps**
1. Mount `filer-intake` as the Intake tab: watched folders, manual drop, rule engine, profiles, preview/confirm, collisions (Keep both / Overwrite / Skip, labelled Skip / Keep both / Replace per UI_SPEC §2), undo last batch, zip import, multi-destination copies (K-1 → entity + shareholder). Confirm runs Filer's `run_commit` inside one `Transaction`. Verify parity with Filer Desktop's UI_SPEC screens at the vendored commit.
2. Scanner: choose TWAIN (via `twain` crate) or WIA (COM via `windows`) — spike both for one day, record choice in QUESTIONS.md Q-02. Scan profiles: duplex, dpi, color mode, deskew, blank-page drop (threshold on ink coverage). Output → intake queue.
3. Barcode cover/separator sheets: generate PDF sheets encoding `{client_id}|{doc_type}|{tax_year}|{split}` as Code128 + QR; "Print cover sheet" from any client; batch of blank separators. Intake decodes with `rxing` on first page of each scanned segment and splits/routes accordingly.
4. Image normalization: JPG/PNG/TIFF/HEIC → PDF (one page per image, auto-rotate from EXIF, deskew, downsample to 300 dpi) before the rule engine sees the file.
5. PDF-printer watcher: Admin setting "Print capture folder"; a setup screen explains configuring Microsoft Print to PDF (or any PDF printer) to save there. Files arriving there enter intake with `source = firm` and a parser profile applied to the filename (UltraTax, Lacerte, CCH, Drake, ProSeries conventions as editable regex profiles in `config/parsers/*.toml`).
6. Badge counts on the Intake tab; silent by default (Filer behavior).

**Done when**
- [x] A 40-page scanned stack with 4 cover sheets splits into 4 documents filed to the right clients with no manual edits — *`vf-fs` `stack::forty_page_stack_with_four_covers_files_four_documents` (from `<data>/Scans`; scanner itself waits on step 2)*
- [x] HEIC phone photo becomes a 1-page PDF and routes by rule — *`vf-fs` `incoming::photo_tests::heic_photo_becomes_a_one_page_pdf_that_routes_by_rule` (CI, feature `heic`)*
- [x] A PDF saved by the system PDF printer into the capture folder with an UltraTax-style filename lands in `<Client>/<Year>/Returns/` with doc type "Return copy — federal" — *`vf-fs` `capture::tests::ultratax_print_capture_files_as_federal_return_copy`; the folder is the template's `Income Tax/{Year}` (CLAUDE.md: UI_SPEC paths are illustrative)*
- [x] Undo last batch restores every file and catalog row — *`vf-fs` `intake::tests::commit_records_catalog_and_undo_restores_files_and_rows`*

---

## P03 — OCR engine trait, Tesseract, FTS5 search, searchable-PDF write-back

**Goal:** Everything in the cabinet is full-text searchable within seconds of arrival.

**Steps**
1. `vf-ocr::OcrEngine` trait: `fn recognize(page: &Image) -> OcrPage { words: Vec<(bbox, text, conf)> }`. Tesseract impl via `leptess` with bundled `eng.traineddata`. Engine chosen from config; only one impl in v1.
2. Job queue: `vf-ocr::Queue` on a `tokio` worker pool (N = cores − 1), persistent job table in catalog, priority (intake > backlog), resumable after crash.
3. Page render via `pdfium-render` at 300 dpi gray → OCR → write invisible text layer back into the PDF (`lopdf`), preserving the original as a version if the file had no text layer. Skip pages that already have extractable text.
4. `vf-index`: FTS5 virtual table over (doc_id, page, text) + metadata columns; trigram tokenizer for partial matches; snippet/highlight.
5. Search tab: single box; filters (client, year, doc type, status, source, date range, filed by, tag, has-SSN flag); results with snippet + thumbnail; open at matching page in Viewer. Scope toggle: this folder / this client / cabinet.
6. Saved searches → virtual folders in Browse.
7. Duplicate detection: exact by hash on intake (prompt: skip / keep both / replace); near-dup by OCR text shingle similarity as a background report.
8. `vibe-files ocr backlog` CLI to OCR an imported cabinet.

**Done when**
- [ ] Fixture cabinet fully indexed; a query returns in <50 ms
- [ ] Searching an EIN that appears only in a scanned image finds the document and opens at the right page
- [ ] OCR survives app restart mid-backlog without re-doing finished pages
- [ ] Exact duplicates are caught on intake

---

## P04 — PDF viewer, annotations, tickmarks, stamps, flatten, PDF/A, redaction

**Goal:** Staff never need Acrobat for day-to-day review and delivery prep.

**Steps**
1. Viewer tab: continuous scroll + single page, zoom, rotate view, thumbnail strip, text select/copy, find-in-document, go-to-page. Opens from Browse, Search, Intake preview.
2. Annotations (stored as real PDF annotations): highlight, underline, strikeout, sticky note, text box, freehand, arrow. Tickmark palette (✓ ✗ ⊘ ƒ T/B PY CY and firm-defined glyphs) as stamp annotations with author + date.
3. Stamps: "Client Copy", "Draft", "Filed {date}", "Received {date}", "Reviewed {initials} {date}", firm logo; placement presets; batch-stamp a selection.
4. Flatten (annotations + forms → page content). PDF/A-2b conversion with validation of fonts and color spaces; fallback to rasterize page if a construct can't be converted, with a warning.
5. Redaction: draw-box and select-text modes; **auto-find** SSN / EIN / account-number / DOB patterns via `vf-security::pii`; apply = remove text and image content under boxes (re-render affected regions), strip metadata, write new file as a new document "name (redacted).pdf" linked to the original. **Verify step**: re-extract text and assert no pattern matches; refuse to save otherwise.
6. Page ops reuse `filer-pdf`: combine (bookmark per source), split, rotate, delete/reorder, extract range to a new document in another client.
7. Compress/optimize scanned images (downsample, JBIG2/CCITT where safe).

**Done when**
- [ ] A 300-page scanned return renders the first page in <200 ms and scrolls without stutter
- [ ] Annotations round-trip through Acrobat Reader
- [ ] Auto-find flags every synthetic SSN in the fixture set; after redaction, text extraction finds none, and the audit row records the pattern counts
- [ ] PDF/A output passes veraPDF on the fixture set (run as an optional CI step)

---

## P05 — Versions, audit trail UI, soft delete, undo

**Goal:** Nothing is ever lost and every action is attributable.

**Steps**
1. Versioning: any content change through Transaction stores prior hash in `_Vibe/versions/` (dedupe by hash). Version panel: list, open, restore (as new version), download.
2. Audit tab (Admin): filter by user / client / doc / action / date; export CSV and PDF; chain verify button; weekly scheduled verify with result banner.
3. Soft delete → `_Vibe/trash/` with original path; Trash view; restore; empty trash gated by retention (P07) and admin role.
4. Undo: generalize Filer's "undo last batch" to a per-user undo stack of the last 20 Transactions (not just intake).
5. Integrity sweep: scheduled re-hash of the tree vs catalog; report drift (modified outside app, missing, unknown files) with one-click "adopt" or "re-file".

**Done when**
- [ ] Overwriting a file 5 times yields 5 versions and 1 physical copy per distinct content
- [ ] Audit export for a client reconstructs its full history in order
- [ ] A file edited in Explorer is detected by the sweep and adopted as a new version with audit `external_modify`

---

## P06 — Users, roles, local auth, SQLCipher key handling, PII flags, §7216

**Goal:** Multi-user-safe access and the compliance primitives the retention phase builds on.

**Steps**
1. Users + roles: admin / preparer / reviewer / front desk / read-only. Per-client ACL (allow/deny lists); default allow. Role matrix documented in `docs/ROLES.md`.
2. Login: Windows SID auto-login when the SID is mapped to a user; otherwise username + Argon2 password. Idle lock. TOTP required for admin actions (user mgmt, stale-lock override, empty trash, key export).
3. Keys: catalog key wrapped by DPAPI per Windows user and by the cabinet recovery passphrase (Argon2id). Firm cabinet key (for sync) generated now, stored the same way, exportable only by admin with TOTP — this export is the off-machine escrow copy; no server holds the key.
4. PII detector runs on OCR completion; sets `has_pii` flags and counts per document; Browse shows an "SSN" badge; export/share/print actions warn.
5. §7216 consents: per-client records (scope, signed date, expiry, linked consent PDF). Any action targeting a destination tagged `third_party` (export to folder flagged external) checks for active consent and warns/blocks per config.
6. Audit covers login, failed login, role change, ACL change, key export.

**Done when**
- [ ] A preparer denied on client X cannot see X in Browse, Search, Intake preview, or Viewer, and the fixture test proves it at the `vf-core` query layer, not just the UI
- [ ] DPAPI-wrapped key opens the catalog with no prompt for a mapped Windows user; recovery passphrase opens it on a fresh machine
- [ ] Export of a flagged doc to a `third_party` destination without consent is blocked and audited

---

## P07 — Retention classes, disposition workflow, legal hold, Safeguards appendix

**Goal:** Defensible records lifecycle with evidence for the WISP.

**Steps**
1. Retention classes seeded (editable): Returns & workpapers 7y from tax-year end; client source docs 4y from tax-year end; engagement letters 7y from engagement close; permanent file never; correspondence 4y; notices 7y. Anchor logic in `vf-core::retention`.
2. Monthly "Eligible for disposition" report: list, by client and class, with reviewer approve/defer per line; approved items move to a 30-day hold, then secure delete (overwrite + unlink) of file, versions, OCR cache, thumbnails; tombstone retained.
3. Certificate of destruction PDF per run, filed under cabinet `_Vibe/disposition/` and linked in audit.
4. Legal hold: per client or per document set; freezes disposition and empty-trash; banner in Browse.
5. Safeguards appendix export (Admin): one PDF with cabinet inventory (paths, counts, size), encryption status, user/role list, retention schedule, last integrity verify, last audit verify, last backup/sync time (if P11 present), disposition runs in period. Formatted to drop into the WISP Builder output.

**Done when**
- [ ] Fixture docs with synthetic dates land in the right disposition buckets
- [ ] A held client's documents never appear in the eligible report
- [ ] Secure delete leaves no copy in versions/cache/thumbs; the tombstone and certificate remain
- [ ] Safeguards appendix renders with all sections populated

---

## P08 — Local-pattern extraction and auto-file

**Goal:** Common forms file themselves even when the filename carries no client ID.

**Steps**
1. `vf-extract`: per-form recognizers (W-2, 1099-INT/DIV/B/R/NEC/MISC, 1098, K-1 1065/1120S/1041, SSA-1099) using OCR words + bbox: title detection, then zone/regex extraction of payer name, payer EIN, recipient name, recipient TIN (last 4 only stored), tax year, box totals where cheap.
2. Confidence scoring; results into `document_extract` table (new migration) and metadata panel.
3. Auto-file hook in the rule engine: if no client match from filename, match recipient TIN-last-4 + name against client master (and relationships for K-1s); propose client + doc type + year in the intake preview with confidence; never auto-confirm below threshold.
4. Correction feedback: when a user overrides, record it; a simple per-firm alias table (payer name → client) improves future matches.
5. Fixture: synthetic form PDFs rendered from templates for each recognizer with ground-truth JSON.

**Done when**
- [ ] ≥95% correct client + year + doc type on the synthetic fixture set
- [ ] A K-1 with no filename ID proposes both entity and shareholder destinations
- [ ] Extraction never writes a full TIN anywhere (test asserts)

---

## P09 — LAN tier: catalog lock protocol, check-out/check-in

**Goal:** 2–10 workstations on one UNC cabinet without corruption.

**Steps**
1. `vf-fs::lanlock`: `_Vibe/catalog.lock` atomic create (`CREATE_NEW`), JSON lease {device, user, since, lease_until, generation}; heartbeat; takeover after expiry with audit; writer bumps generation on each committed Transaction.
2. Reader mode: workstations without the lock open a snapshot copy of `catalog.db` (copied when generation changes), read-only; all writes queue a Transaction request that waits for the lock (bounded wait, then "cabinet busy" UI).
3. Check-out/check-in per document: lock row + `_Vibe/locks/<uuid>.lock`; Browse shows lock badge with user/since; opening a locked doc is read-only for others; check-in on save/close or explicit; stale lock override (admin, TOTP, audit).
4. Device registry (Admin): name, mode, last seen; revoke.
5. Soak test harness: 4 processes hammering one fixture cabinet on a loopback SMB share for 10 minutes; zero corruption, zero lost Transactions.

**Done when**
- [ ] Soak test passes; `PRAGMA integrity_check` ok after every run
- [ ] Writer crash mid-lease → another workstation takes over within 45 s with an audit row
- [ ] Two users cannot both check out the same document

---

## P10 — Folder-tree import wizard

**Goal:** Adopt any existing folder tree (FileCenter cabinets, legacy firm trees) without moving files.

**Steps**
1. Wizard: pick root; scan; propose client folder detection (regex over folder names, CSV client list match, fuzzy); map year folders; map doc type by folder name/filename keywords; preview as a table with confidence.
2. Apply = write sentinels, catalog rows, optional rename to the firm template (preview + undo), queue OCR backlog.
3. Report of unmapped folders/files for manual handling.

**Done when**
- [ ] A FileCenter-style `Cabinet/Drawer/Folder` tree of 2,000 files imports with ≥90% auto-mapped and a reviewable remainder

---

## P11 — B2 two-way sync on the Vibe Files layout

**Goal:** The cabinet mirrors to B2 in the `docs/BUCKET_LAYOUT.md` layout; changes from other devices come back; conflicts are impossible or visible.

**Steps**
1. Read `docs/BUCKET_LAYOUT.md`. Implement `vf-sync::layout` exactly; contract tests against a MinIO container seeded from the fixture cabinet.
2. Encryption: per-object XChaCha20-Poly1305 with the firm cabinet key (P06); manifest encrypted too; key id in object metadata for rotation.
3. Push: from `sync_pending` (fed by every Transaction), content-addressed upload (skip if hash exists), multipart >100 MB, resumable, bandwidth window, retry with backoff. Manifest update last.
4. Pull: poll manifest rev; new/changed objects → download → verify hash → Transaction `sync_pull` (new document, new version, tombstone, lock), filed with the vendored Filer move primitives.
5. Locks in bucket: check-out writes a lock object; pull refuses to overwrite a locally checked-out doc; push refuses if a remote lock by another device exists.
6. Conflict: unlocked collision → keep local as `name (conflict from <device>).ext`, pull remote as the canonical, raise Conflicts item with side-by-side open and "keep mine / keep theirs / keep both".
7. Deletes → tombstone in manifest; object removal only via disposition (P07) after all devices have acknowledged.
8. Sync tab: status, pending bytes, last push/pull, errors, per-client state, pause/resume, verify now (sampled download + hash), restore client/cabinet to path.
9. Object Lock option (B2 immutability window) exposed as a cabinet setting with explanation.
10. Browse integration (per UI_SPEC §1): `Sync to B2 ▾` toolbar menu scoped to the current client — push now, pull latest, verify (sampled), show in Sync tab; per-client sync status pill in the breadcrumb row (pending count / in sync / conflict / pulling) that deep-links to the Sync tab filtered to that client. Both hidden when sync is not configured.

**Done when**
- [ ] Round-trip: file in on PC-A → appears on PC-B with identical hash and metadata
- [ ] A client folder written by PC-A's Filer engine (sentinel + template tree) restores on PC-B with the same `client_id`, same folder name, and passes the Filer root scan as `Bound`
- [ ] Forced collision produces a conflict copy and a Conflicts item; nothing overwritten
- [ ] Bare PC + recovery passphrase + bucket credentials restores the fixture cabinet to byte-identical files
- [ ] From Browse, `Sync to B2 ▾ → Push this client now` pushes only that client's pending objects and the breadcrumb pill goes from `4 pending` to `in sync` without leaving the tab

---

## P12 — Remote mode

**Goal:** A laptop with no cabinet copy works from the bucket.

**Steps**
1. Device mode `remote`: pull catalog snapshot + manifest + thumbnails; no file bytes.
2. On open: fetch object → decrypt → verify → `cache/remote/` (LRU, configurable cap, default 5 GB); open from cache; edits go through Transaction → push.
3. "Pin client for offline" keeps all of a client's current versions resident; unpin evicts. Exposed in the Sync tab per-client table **and** in Browse's `Sync to B2 ▾` menu (`Pin client on this device` / `Unpin`); the Browse breadcrumb pill shows `client pinned on <device>`.
4. Search in remote mode = metadata/filename only; UI banner states full-text requires a local cabinet. (Full-text remote is a v2 item.)
5. Offline behavior: queue writes; reconcile on reconnect via P11 rules.
6. Thumbnails generated by the filing workstation and synced as small objects so remote Browse is visual.

**Done when**
- [ ] Fresh laptop in remote mode browses the fixture cabinet, opens a doc in <3 s on a 20 Mbps link, annotates, and the change appears on the office PC
- [ ] Cache never exceeds cap; pinned client survives eviction
- [ ] Offline edits sync cleanly on reconnect

---

## P13 — Hardening, performance, release

**Steps**
1. Performance pass: 50,000-document synthetic cabinet; Browse, Search, startup under targets (startup <2 s, search <100 ms, tree expand <100 ms).
2. Crash safety: kill -9 during intake, OCR, redaction, sync — no partial files, no orphan rows; fuzz Transaction rollback.
3. Memory/handle leaks over an 8-hour soak.
4. Startup self-checks: catalog integrity, audit chain, cabinet reachable, lock sanity, sync state.
5. `vibe-files` CLI complete: rebuild, ocr backlog, audit verify, sync push/pull/status, export client zip (files + audit extract + manifest).
6. Portable release build (static CRT, pdfium and tesseract data bundled), SHA-256 of the exe published, in-app update check against the licensing endpoint (no auto-install in v1).
7. `docs/OPERATIONS.md`: setup on a new office, LAN share setup, remote laptop setup, key escrow procedure, disaster recovery from bucket.

**Done when**
- [ ] All targets met and recorded in STATE.md
- [ ] Kisaes runs it for real on the live cabinet for two weeks with no data incident

---

## Addenda (v2 candidates — not scheduled)

- PA-01 Email ingestion (.msg/.eml split, body→PDF) and Outlook add-in
- PA-02 Office check-in watcher + text indexing of docx/xlsx
- PA-03 ONNX OCR engine (PaddleOCR/docTR) behind the existing trait
- PA-04 Full-text search in remote mode (synced index shards)
- PA-05 Bates numbering, page-image compare, AcroForm fill
- PA-06 Vibe Auth OIDC login
- PA-07 Explorer shell namespace extension
- PA-08 FileCabinet CS and SmartVault importers
- PA-09 Appliance tier (Axum/Postgres server, thin client)
- PA-10 Sellable packaging: installer, trial, licensing editions, docs site
- PA-11 Practice Manager interop (portal uploads → Intake, PM reading the bucket, shared key). Out of v1 by owner decision 2026-11-02; would need its own layout mapping, not a change to `BUCKET_LAYOUT.md` v1
- PA-12 GitHub CI and portable release build. **Scheduled by owner 2026-11-02.** `.github/workflows/ci.yml` runs the Windows gate (fmt, clippy `-D warnings`, test) on `windows-latest` for every push to `main` and every PR. `.github/workflows/release.yml` builds `vibe-files.exe` (release, static CRT) and ships it with `pdfium.dll`, licenses and `packaging/README.txt` as `vibe-files-<ver>-windows-x64.zip` plus a `.sha256.txt`; a `vX.Y.Z` tag matching the workspace version publishes a GitHub pre-release, Run workflow gives a downloadable artifact. Portable zip only: an installer stays in PA-10. Covers the build half of P13 step 6 early; tesseract data and the update check stay in P13.
