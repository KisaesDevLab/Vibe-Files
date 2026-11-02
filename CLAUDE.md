# Vibe Files — CLAUDE.md

Vibe Files is a Windows desktop document management system for small CPA/tax firms, built in Rust (egui). It is the cabinet layer over a plain client folder tree: browse, intake, OCR, search, PDF tools, versions, audit, retention, and two-way sync to Backblaze B2. Its folder tree, sentinel, and document-moving engine come from Vibe Filer Desktop (`KisaesDevLab/Vibe-Filer-Desktop`). Practice Manager's Filer module is **not** a source or a dependency (owner 2026-11-02); ignore it.

This file is the standing brief. Read it fully before every session. Then read STATE.md, then the current phase in PHASES.md.

## Who this is for

- v1: internal use at Kisaes LLC. One office, a handful of workstations on a LAN share, plus remote-mode laptops.
- v2: sellable to other small firms. Nothing in v1 may make v2 impossible, but v1 does not pay for v2 polish (installer, trial, docs).
- License: PolyForm Internal Use 1.0.0 for the repo. Vendored Filer Desktop crates keep their existing headers.

## Non-negotiable principles

1. **Files are plain files in a plain Windows folder tree.** Any PDF or Office file in the cabinet opens in Explorer without Vibe Files. No blob store, no proprietary container.
2. **The catalog is a rebuildable index.** SQLite (SQLCipher) at `<data>/catalog.db`. `vibe-files rebuild` walks the tree, reads sentinels, re-hashes, and reproduces the catalog minus history that only ever lived in the catalog (audit trail, versions sidecar manifests are on disk too — see §Layout).
3. **Offline first.** Every v1 feature works with no network. B2 sync and remote mode are additive.
4. **Portable exe.** `vibe-files.exe` + `<data>/` beside it. No registry writes except the optional Explorer context-menu entry, which the user opts into.
5. **Every mutation is a Transaction.** One type, `core::tx::Transaction`, writes the file change, the catalog row, the audit row, and the undo record together or not at all.
6. **Sync-aware from P00.** The catalog schema carries content hashes, tombstones, lock records, and device IDs from the first migration, even though sync lands in P11. No schema rewrite for sync.
7. **No telemetry. No cloud calls except the user's own B2 endpoint and the licensing check.**
8. **No Practice Manager dependency.** Filer Desktop is the only upstream. Nothing in v1 reads Practice Manager source, documents, or bucket objects.

## Vocabulary

| Term | Meaning |
|---|---|
| Cabinet | A root path (local or UNC) holding client folders. A firm usually has one. |
| Client folder | A folder bound to a client by `_Vibe/client.json` (SentinelV1 exactly as Filer Desktop's `sentinel.rs` reads and writes it). |
| Document | A cataloged file. Identity = catalog UUID; content identity = SHA-256. |
| Doc type | Firm-editable catalog of document kinds (W-2, K-1, 8879, engagement letter, …) carrying default folder, retention class, client-visible flag. |
| Tax year | First-class attribute on every document, independent of folder name. |
| Permanent file | Documents with no tax year (formation docs, EIN letters, elections). |
| Intake | The Filer Desktop engine: watched inbox folders, rule engine, preview/confirm queue, undo. |
| Version | A prior content hash of a document, kept in `_Vibe/versions/`. |
| Tombstone | A deleted document's catalog row, kept for sync and retention. |
| Lock | Check-out record. Lives in the catalog and in `_Vibe/locks/` (LAN) and in the bucket (sync). |
| Remote mode | Workstation holding catalog + thumbnails but fetching file bytes from B2 on open. |

## Layout on disk

```
<cabinet>/
  _Vibe/
    cabinet.json            # cabinet id, firm id, schema version, B2 layout version
    locks/<doc-uuid>.lock   # LAN check-out locks (JSON: user, machine, since, lease)
    versions/<hash[0..2]>/<hash>   # content-addressed prior versions
    versions/<hash[0..2]>/<hash>.json   # sidecar: [{rel_path, created, created_by, device_id, reason}] so rebuild restores version rows
    audit/<yyyy-mm>.jsonl   # audit trail, hash-chained, append-only (also in catalog)
    trash/                  # soft-deleted files, original path recorded in catalog
  <Client> (<ID>)/          # Filer template top folder `{Client} ({ID})`
    _Vibe/client.json       # SentinelV1
    <folders per the cabinet template>   # default = Filer's assets/default_template.json:
                            #   Income Tax/{Year} · Bookkeeping/{Year} · Payroll/{Year} · Financials/{Year}
                            #   Correspondence/{Year} · Workpapers & Support/{Year} · Sales Tax/{Year}
                            #   Permanent/{Setup, Stock Basis} · Other/{Practice/{Year}, {Year}} · Not Yet Filed
<data>/                     # beside the exe
  catalog.db                # SQLCipher
  config.toml
  cache/thumbs/, cache/ocr/, cache/remote/   # remote-mode LRU cache
  logs/
```

Folder naming, templates, and the filename token system (`{Client}`, `{ID}`, `{Year}`, `{DocType}`, `{Rest}`, `{Ext}`, CSV columns, `{Copy.*}`) are Filer Desktop's; do not reinvent them. Read `crates/filer-*/VENDORED.md` first. UI_SPEC path examples such as `Anderson Farms LLC_1042 / 2025 / Source Docs` are illustrative only; the real tree is whatever the cabinet template renders.

## Document moves — source of truth

Every move, copy, collision, and undo of a document is based on the **most recent Vibe Filer Desktop repo**: `KisaesDevLab/Vibe-Filer-Desktop` @ `master` `ea4e7267797ee869f05c3e5745700e13910a2e5b` (v0.2.4, 2026-11-02). Do not write a second move engine. `vf-fs` and `vf-core::tx` wrap the vendored code; they do not replace it.

What is taken from it as-is (`src/commit.rs`, `src/undo.rs`, `src/plan.rs`, `src/oplog.rs`):

- `move_no_replace`: `MoveFileExW(MOVEFILE_WRITE_THROUGH)`, never replaces an existing file. Long source paths get the `\\?\` / `\\?\UNC\` prefix (`verbatim`).
- `transfer`: same volume = rename. Cross volume (`ERROR_NOT_SAME_DEVICE` 17) = copy to `~vfd-partial-<uuid>-<name>` beside the destination, `sync_all`, SHA-256 compare, rename into place, **then** delete the source. If the source can't be deleted, the verified copy is taken back so nothing is duplicated.
- `copy_verified`: same temp + hash + rename, source never touched. Multi-destination copies (K-1 → entity + shareholder) are written and verified **before** the primary move; a later failure rolls the copies back.
- Collisions: `Keep both` (default, ` (2)` suffix via `free_path`) · `Overwrite` · `Skip`, chosen per row at preview. A destination that appears after preview is never overwritten. Overwrite keeps the old file first (see QUESTIONS Q-17 for where it goes in Vibe Files).
- Commit continues past a failed file (Filer X75) and reports per-file outcomes; destination paths over 240 chars are refused (X104).
- Undo blockers and rules: refuse if a filed file's size changed or it is gone; copies are deleted only if their hash still matches, modified copies are left and reported; only folders the batch created are removed, and only if empty. Never `remove_dir_all`.
- Every write is checked to be inside the cabinet root or an inbox (canonicalize + prefix), with Filer's X53 exception for hand-added files.

What Vibe Files changes on top (in the wrapper, not in the vendored file):

- Filer's `ops.jsonl` records become audit rows (and `_Vibe/audit/*.jsonl`); `last_batch.json` becomes the Transaction's undo record; the per-user 20-deep undo stack (P05) is built from those records.
- Each filed file also gets its catalog `document` row and a `sync_pending` row inside the same Transaction.

Upstream is still moving (0.2.x pre-releases, fresh-VM acceptance open). At P00 step 2, check `master` again; if it is newer than the commit above, vendor the newer HEAD and record it. After that, re-vendoring is an addendum step (PA-xx), never a silent update.

## Workspace

```
Cargo.toml (workspace)
crates/
  filer-core/        # vendored: config, clients, sentinel, template, relationships, rules, rulebuild, parse
  filer-pdf/         # vendored: pdf (merge/split/text/thumbnails), split (cut-point finder)
  filer-intake/      # vendored: intake, zipimport, plan, commit, undo, oplog, recovery, worker + ui/* (the Intake tab)
  vf-core/           # catalog schema, document model, Transaction, doc types, retention, audit, locks
  vf-fs/             # tree walker, LAN lock protocol, trash, secure delete; moves/copies call filer-intake::commit (never reimplemented)
  vf-pdf/            # viewer render (pdfium), annotate, stamp, flatten, PDF/A, redact
  vf-ocr/            # OcrEngine trait, Tesseract impl, job queue, searchable-PDF write-back
  vf-index/          # FTS5 full-text + metadata search, saved searches
  vf-ingest/         # scanner (TWAIN/WIA), barcode sheets, image→PDF, PDF-printer watcher
  vf-extract/        # local-pattern extraction (W-2/1099/K-1/1098) → metadata, auto-file
  vf-security/       # users, roles, local auth, Windows auto-login, DPAPI, PII detectors, §7216
  vf-sync/           # B2/S3 two-way sync, shared layout, conflicts, remote mode fetch
  vf-events/         # outbox JSON events, CLI
  vf-app/            # egui shell
docs/
  BUCKET_LAYOUT.md           # Vibe Files' own B2 layout, mirroring the Filer Desktop client tree — the contract vf-sync implements
  SENTINEL_V1.md             # written from Filer Desktop's src/sentinel.rs at the vendored commit
  UI_SPEC.md                 # screens (seeded from Filer Desktop's 11-screen spec)
```

`vf-core` has **no** UI or network dependency. `vf-app` depends on everything; nothing depends on `vf-app`.

## Stack

- Rust stable, **2024 edition** (matches the vendored Filer code), `cargo clippy --all-targets -- -D warnings` clean, `cargo fmt` enforced.
- UI: `eframe`/`egui`/`egui_extras` **0.37** (pinned to the vendored Filer version so its UI mounts unchanged), `rfd`, `arboard`. Workspace `[workspace.dependencies]` pins every crate the vendored code uses at Filer's `Cargo.lock` versions.
- Catalog: `rusqlite` with `bundled-sqlcipher`, WAL on local disk only. Migrations by `PRAGMA user_version`, forward-only, backup before migrate.
- PDF: `pdfium-render` (render/text), `lopdf` (structure edits), `pdf-writer` (new content).
- OCR: `leptess` behind `vf_ocr::OcrEngine`. ONNX slot reserved; do not add `ort` in v1.
- Sync: `object_store` (S3 feature) against B2's S3 endpoint; `chacha20poly1305` for per-object encryption; `tokio` runtime confined to `vf-sync` and `vf-ocr` workers.
- Barcodes: `rxing`. Images: `image`, `libheif-rs` (HEIC). Hashing: `sha2`. IDs: `uuid` v7. Time: `time` (UTC in storage, local in UI).
- Windows FFI via the `windows` crate in new crates; vendored Filer code keeps `windows-sys` (its `MoveFileExW` call). Vendored code also keeps `chrono`, `uuid` v4 batch ids, `notify`, `zip`, `strsim`, `clap`; new catalog ids are uuid v7 and new code uses `time`. TWAIN via `twain` bindings or WIA COM; pick one in P02 and record in QUESTIONS.md.

## Multi-user rules (LAN tier)

- SQLite over SMB is unsafe. The catalog on a UNC cabinet is opened by **one writer at a time** holding `_Vibe/catalog.lock` (atomic create, lease 30 s, heartbeat every 10 s). Readers open a snapshot copy refreshed when the writer's generation counter changes.
- Document-level contention is handled by check-out locks, not by the catalog lock.
- Stale lock override is an admin action and writes an audit row.
- Every workstation has a `device_id` (uuid v7, stored in `config.toml`). Every catalog row that can change carries `device_id` and `rev`.

## Sync rules (P11–P12)

- Bucket layout is Vibe Files' own, in `docs/BUCKET_LAYOUT.md` (written in P00 step 3). Client paths in the bucket mirror the Filer Desktop client tree (`{Client} ({ID})/…` per the cabinet template) and are keyed by the sentinel's `client_id`, so a folder rename is a manifest change only. Change the layout only by bumping `layout_ver` in `cabinet.json` with a migration.
- Objects are content-addressed by SHA-256. Path↔hash mapping lives in the manifest. Renames/moves update the manifest only.
- Every object is encrypted client-side with the firm cabinet key before upload. The key is held only by the firm (DPAPI-wrapped on workstations, recovery passphrase, admin export with TOTP); no server holds it. Key derivation and escrow are in `vf-security::keys`.
- Conflict rule: a lock in the bucket prevents concurrent edit. An unlocked collision writes `name (conflict from <device-name>).ext` beside the original and raises a Conflicts-tab item. Never silently overwrite.
- Deletes are tombstones in the manifest; objects are removed only when retention (vf-core) clears them.
- Remote mode: pull catalog + thumbnails + manifest; fetch bytes on open into `cache/remote/` (LRU, size cap in config); "pin client" keeps a client resident. Full-text search is unavailable in remote mode in v1; metadata search only. The UI must say so.

## Security rules

- Catalog is always SQLCipher. Key wrapped by Windows DPAPI for the logged-in user, with a recovery passphrase set at cabinet creation.
- PII detector (`vf-security::pii`) flags SSN/EIN/account-number patterns on OCR completion. Redaction is true content removal (`vf-pdf::redact`), never a black rectangle over live text. Verify by re-extracting text after redaction; fail loudly if the pattern still matches.
- §7216: a client may hold consent records (date, scope, expiry, scanned PDF). Any export/share action to a destination tagged `third_party` checks for active consent and warns.
- Audit rows are `{seq, ts, user, device, action, doc, detail, prev_hash, hash}`. Verify-chain runs on startup and weekly.

## Conventions

- Conventional commits: `feat(vf-pdf): redaction with pattern verify`, `fix(vf-sync): …`, `chore(deps): …`. One phase step per commit where practical.
- Tests: unit tests beside code; integration tests in `crates/*/tests/`; a fixture cabinet in `fixtures/cabinet/` with synthetic PDFs (no real client data, ever).
- Errors: `thiserror` in crates, `anyhow` only in `vf-app` and CLI.
- Logging: `tracing`, daily rolling file in `<data>/logs/`, no PII in logs.
- UI: tabs in this order — **Browse · Intake · Search · Viewer · Sync · Admin**. Follow `docs/UI_SPEC.md`. Preview + confirm before any batch mutation. Undo last batch always available.
- Never write real client data into fixtures, tests, logs, or commit messages.

## Session protocol

1. Read CLAUDE.md, STATE.md, the current phase in PHASES.md, and QUESTIONS.md.
2. If an open question blocks the phase, answer it with the recorded default and note the assumption in STATE.md; if no default is recorded, stop and ask.
3. Work the phase's steps in order. Commit per step. Keep `cargo test` and clippy green at every commit.
4. Update STATE.md at the end of every session: what shipped, what's half-done, exact next step.
5. Do not start the next phase until the current phase's "Done when" list is fully checked.
6. New scope arrives as an addendum phase (PA-xx) at the end of PHASES.md, never as edits to a completed phase.
