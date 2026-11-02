# Vibe Files — UI_SPEC.md

Derived from the approved Design canvas "Vibe Files UI" (6 screens, 1440×900 preview). This is the layout and behavior contract for `vf-app`. Where egui cannot match a web idiom exactly, match the information hierarchy and the interaction, not the pixels.

## Global chrome (every tab)

```
┌ Header 44px, #161A20 bg, white text ───────────────────────────────────────────┐
│ [logo] Vibe Files   Kisaes LLC · \\KISAES-NAS\Cabinet        [LAN · catalog writer] kwk · admin [🔒] │
├ Tab bar 44px, white, bottom border #D5D9E0 ────────────────────────────────────┤
│ Browse | Intake (7) | Search | Viewer | Sync | Admin                            │
├ Body: 3-pane (left aside · main · right aside) or full-width (Sync) ────────────┤
│                                                                                │
├ Footer 28px, #F4F6F9 ──────────────────────────────────────────────────────────┤
│ status segments … · right-aligned health indicator                             │
└────────────────────────────────────────────────────────────────────────────────┘
```

- **Header** shows firm name, cabinet path (mono), device mode pill (`LAN · catalog writer` / `LAN · reader` / `Local` / `Remote · 3.1/5 GB cache`), current user + role, lock-session button.
- **Tab bar**: active tab has a 2px accent underline. Intake tab carries a badge with pending-queue count (silent — no toast, no sound).
- **Footer** segments are per-tab (see each screen) but always end with a right-aligned health item (audit chain verified date on Browse; full-text availability on Search; etc.).
- **Accent color** is a firm setting; default `#1F56A8`. Options offered: `#1F56A8` blue, `#1D5C38` green, `#7A4F07` brown, `#161A20` ink.

### Type and color tokens

| Token | Value |
|---|---|
| Font | IBM Plex Sans (UI), IBM Plex Mono (filenames, hashes, IDs, paths, times) — bundle both |
| Base size | 13px; labels 11px uppercase tracked; section titles 14px/600; page titles 16px/600 |
| Ink | `#161A20` text · `#3F4753` secondary · `#5B6470` muted · `#B5BCC6` on-dark muted |
| Surfaces | `#EEF0F3` app bg · `#FFFFFF` panes · `#F4F6F9` hover/footer · `#FAFBFC` sticky table header · `#DEE2E8` viewer canvas |
| Borders | `#D5D9E0` pane · `#E9ECF0` row · `#C9CFD8` control |
| Selection | row `#E6EEF9`, text `#1F56A8` |
| Status pills | Received `#EEF0F3/#3F4753` · In work `#FBEFD5/#7A4F07` · Final `#DDF2E4/#1D5C38` · Delivered `#E6EEF9/#1F56A8` · SSN/error `#FBE3E3/#9A1B1B` · Lock `#FBEFD5/#7A4F07` |
| Match confidence pills | High `#DDF2E4/#1D5C38` · Mid `#FBEFD5/#7A4F07` · Low/duplicate `#FBE3E3/#9A1B1B` |
| Health dots | ok `#2E7D4F` · warn `#B7791F` · error `#9A1B1B` · info `#1F56A8` |
| Folder icons | client folder `#D9B46B`, subfolder `#E6D3A3` |

Controls: 32px height, 4px radius, 1px `#C9CFD8` border; primary = accent fill, white text; icon buttons 32×32; destructive primary (redaction apply) = `#9A1B1B` fill. Minimum hit target 44px for anything touch-ish (tabs, list rows in panels).

---

## 1. Browse (default tab)

Three panes: tree 272px · grid flexible · inspector 320px.

**Left — tree**
- Section **Views**: Recent · Pinned · Needs doc type (count, warn icon) · Needs tax year (count, warn icon) · Received this week (count) · Saved searches (one row each, count).
- Section **Cabinet**: client filter box ("Find client by name or ID", shows client count), then clients as collapsible rows: `[chevron][folder] Client Name …… ID(mono)`. Expanded client shows `Permanent`, then years descending, each with doc count; expanded year shows doc-type folders from the template (Source Docs · Returns · Workpapers · Correspondence …) with counts. Current folder highlighted with selection bg + accent text.

**Main — grid**
- Toolbar: `+ New client` (primary) · `Scan to here` · `Print cover sheet` · divider · `Rename…` · `Bulk edit` · `Check out` · `Open in Explorer` · divider · **`Sync to B2 ▾`** (menu, scoped to the current client or selection) · right-aligned `Filter this folder` search.
  - `Sync to B2 ▾` menu items: `Push this client now` · `Pull latest for this client` · `Pin client on this device` / `Unpin` (remote mode) · `Verify this client (sampled)` · `Show in Sync tab`. Items are disabled with a tooltip when sync is not configured; the whole button is hidden until P11 ships.
- Breadcrumb: `Cabinet / Anderson Farms LLC_1042 / 2025 / Source Docs` with right-aligned group: **sync status pill** (`● B2: 4 pending · client pinned on KWK-LAPTOP`; variants `● B2: in sync`, `● B2: conflict — resolve`, `● B2: pulling 2`, hidden when sync not configured) then `17 documents · 2 selected`. Clicking the pill opens the Sync tab filtered to this client.
- Table columns: ☐ · Name (mono; inline pills: `SSN`, `🔒 user` lock, `Needs doc type`) · Doc type · Year · Status pill · Size (mono) · Modified · Filed by (user / `scanner` / `sync`) · Hash short (mono muted). Sticky header, hover row bg, selected rows `#E6EEF9`. Rows needing attention render name in warn color.
- Double-click: PDF → Viewer tab; Office file → external app (check-out prompt if LAN).

**Right — inspector**
- Header: `N documents selected` + `Undo last` button.
- Thumbnail strip of selected docs (72×94).
- 2×2 fields: Doc type (shows `Mixed (2)` when heterogeneous) · Tax year (incl. `Permanent`) · Status · Source (Client / Firm / Government / Third party); Tags text field.
- Primary button: **Apply to N (one transaction)** — wording is intentional; keep it.
- Single-doc detail block: filename; key/value list — SHA-256, Versions (link, last date), Lock, PII (`SSN ×1 found by OCR`), Retention (`Source docs · 4y → Apr 2030`), §7216 (`Consent on file to 12/2026`), Path.
- Recent activity: last 3 audit rows for the doc, `HH:MM  text`.

**Footer**: `2,418 documents · 5 clients` · `Catalog: writer · this PC` · `OCR queue: 3` · `Sync: idle · last push 4:12 PM` · right: `● Audit chain verified Oct 29`.

---

## 2. Intake

Three panes: sources 272px · queue flexible · item inspector 360px.

**Left — sources**
- **Queue**: All pending (n) · Collisions (n, red) · Low confidence (n).
- **Watched folders**: Scans inbox · Print capture — each with count; dashed drop zone `Drop files or a .zip here`.
- **Scanner**: status dot + device name (`Fujitsu fi-8170 · ready`); Profile select (`Duplex · 300 dpi · gray · drop blanks`, …); primary **Scan stack**.
- **Barcode sheets**: `Cover sheet for client…` · `Print 10 blank separators`.

**Main — Preview & confirm**
- Title row: `Preview & confirm` · muted `7 items · 1 collision · nothing moves until you confirm` · right: `Re-run rules` · `Edit profiles`.
- Table: ☐ · Incoming file (mono name + muted sub-line: `pages 1–3 · barcode cover sheet` / `2 destinations (K-1 rule)` / `UltraTax filename profile` / `1 page · EXIF rotate · deskew` / `4 pages · no barcode · no ID in name` / red `Collision: same hash already filed Oct 20`) · Source pill (Scanner / Print capture / Scans inbox / Manual) · Client (name + mono ID; K-1 rows show second line `+ Delgado, Maria 1150 (partner 40%)`) · Doc type · Year · Destination (mono relative path; two lines for multi-destination; `Pick client…` in warn color when unresolved; inline `Skip / Keep both / Replace` buttons on collision rows) · Match pill (`Barcode` / `Rule` / `Parser` high; `Extract 82%` mid; `Low 41%` / `Duplicate` low).
- Low-confidence and unresolved rows are unchecked by default.
- Footer bar (sticky): primary **Confirm N selected** (36px) · `Undo last batch (4 files, 2:14 PM)` · muted `Each confirm is one transaction: file moves, catalog rows, audit, undo record.`

**Right — item inspector**
- Title: file + page range. Page preview (8.5×11 aspect, max 300px) with `◂ Page  1 / 3  Page ▸`.
- **Decoded cover sheet**: Client · Doc type · Tax year · Split (`new document at this sheet`).
- **Extracted (local patterns)**: Payer · Payer EIN (`43-••••912`) · Recipient TIN (`•••-••-4471 (last 4 only)`) · Box 1. Never show a full TIN.
- Override fields 2×2: Client · Doc type · Tax year · Status. Button `Open full preview in Viewer`.

**Footer**: `Watching 3 folders` · `Last batch: 4 files at 2:14 PM by jmiller` · `OCR queue: 3 (intake first)` · right `Rules: 38 active · 2 profiles`.

---

## 3. Search

Top band (white, 20px padding) over two panes: results flexible · facets 272px.

**Top band**
- 44px search box with query (`43-1920912 year:2025` — supports `field:value` tokens) and mono elapsed time (`38 ms`).
- Scope segmented control: `This folder` · `This client` · `Cabinet` (active = accent fill).
- `Save search…` button.
- Filter chips row: `Year: 2025 ×` (active) · Client · Doc type · Status · Source · Date range · Filed by · Tag · Has SSN.

**Main — results**
- Count row: `**6 documents** match · full text + metadata` · right `Sort: relevance`.
- Result card: thumbnail 56×72 · filename (mono, 500) + status pill (+ SSN pill) · snippet with `<mark>` highlight, muted · meta row: `Client · Year · Folder` · doc type · `Match on page N (OCR text)` / `(scanned image, OCR)` / `Metadata match` / `Office file — opens in Excel` · `Filed date by user`. Related-but-filtered-out results appear with note `Outside Year filter — shown as related`.
- Click → Viewer at matching page.

**Right — facets**: Client · Doc type · Year (incl. Permanent) · Saved searches, each row with count; active facet highlighted.

**Footer**: `Index: 2,418 docs · 31,206 pages · up to date` · `OCR queue: 3` · right `Local cabinet — full-text search available` (remote mode: `Remote mode — metadata search only`).

---

## 4. Viewer

Toolbar + three panes: thumbnails 112px · canvas flexible (`#DEE2E8`) · tool panel 320px.

**Toolbar**
- Left: filename (mono, ellipsis) · status pill · `SSN ×3` pill · muted `v3 · checked out to you`.
- Right: `−` `100%` `+` · `Fit width` · rotate · divider · `Find in document` (180px) · divider · `◂ Page [1] / 2 ▸` · divider · `Print` · `Pages…` · `Flatten` · `PDF/A` · primary **Check in & save**.

**Left — thumbnails**: numbered page tiles, current page has 2px accent border.

**Canvas**: page centered, max 640px wide at 100%, drop shadow. Annotations render as: redaction box = solid black with 2px red outline + label `SSN · redact` (pending state); highlight = `#FFE89A`; tickmark = 28px circle `#DDF2E4/#1D5C38` glyph with tooltip `Tickmark: agreed, kwk`; stamp = rotated accent-outlined text (`RECEIVED 10/29/2025`); note = yellow square + author/text/time caption.

**Right — tool panel** with three sub-tabs: **Annotate** · **Stamps** · **Redact** (active underline accent).
- *Redact*: Mode list (Draw box · Select text · Auto-find patterns); **Auto-find results** list with checkboxes (`SSN · page 1 · box a  •••-••-4471`; EIN rows default unchecked with hint `(payer — usually keep)`); `+ Account numbers` `+ Dates of birth` chips; explainer card ("What apply does": removes content under boxes, strips metadata, writes `… (redacted).pdf` linked to original; re-extracts and refuses to save if any pattern still matches; counts go to audit); destructive primary **Apply N redactions & verify** (40px, `#9A1B1B`). Below: **Tickmarks** palette `✓ ✗ ⊘ ƒ T/B PY CY +` with note "Stamped as real PDF annotations with author and date."
- *Annotate*: highlight / underline / strikeout / note / text / freehand / arrow; color; author shown.
- *Stamps*: preset list (Client Copy · Draft · Filed {date} · Received {date} · Reviewed {initials} {date} · firm logo) + placement presets + `Stamp all pages`.

**Footer**: `2 pages · 184 KB · text layer: OCR (Tesseract, 97% conf)` · `Opened from Search · match on page 1` · right `Changes save as version 4`.

---

## 5. Sync

Full-width page, max 1400px, 20/24px padding.

**Status row**: health dot · `Syncing to B2 · kisaes-cabinet` (16px/600) · muted `Vibe Files layout v1 · client-side encrypted · key id k-02` · right: `Pause` · `Verify now (sampled)` · `Restore client or cabinet…` · primary **Push now**.

**Stat cards** (auto-fit, min 200px): Pending push (`14 files`, `212 MB · multipart on 1 · ETA 3 min`, progress bar) · Last push (`4:12 PM`, `38 objects · manifest rev 2,207`) · Last pull (`4:18 PM`, `2 changes from KWK-LAPTOP`) · Conflicts (`1` in red, `Needs a decision · nothing overwritten`) · Bucket (`41.8 GB`, `2,418 current + 1,106 versions · dedup by hash`) · Object Lock (`Off` in warn, `Suggested: 30-day immutability window`).

**Per-client state table** (left, flexible): Client · State pill with dot (Pushing / In sync / Conflict / Pulling) · Local count · Bucket count · Pending (`4 ↑`, `1 ↑ 1 ↓`, `2 ↓`) · Locks (`1 · tross`) · Pinned on (device) · `Details` / `Resolve` link. Conflict rows tinted `#FFF8F8`. Header note: `Pinned clients stay resident on remote laptops`.

**Right column (380px)**:
- **Conflict card**: pill + `Unlocked collision` · filename · two side-by-side boxes `Mine · this PC` / `Theirs · KWK-LAPTOP` (edited time, user, hash short, size) · explainer `Remote copy is canonical; yours was kept beside it as … (conflict from KWK-LAPTOP).xlsx. Neither was overwritten.` · buttons `Open both` · `Keep mine` · `Keep theirs` · primary **Keep both**.
- **Devices**: rows `dot · name (this PC) · mode pill (LAN · writer / LAN · reader / Remote · 3.1/5 GB cache) · last seen`.
- **Cabinet settings**: toggles `Two-way sync (pull changes from other devices)` · `B2 Object Lock · 30-day window (recommended; versions can't be deleted early)` · Bandwidth window select (`Always` / `6 PM – 7 AM only`) · note `Deletes are tombstones in the manifest; objects are removed only when retention clears them and every device has acknowledged.`

**Footer**: `Endpoint: s3.us-west-004.backblazeb2.com` · `Manifest rev 2,207 · verified sample 50/50 OK on Oct 28` · right `Firm cabinet key held by this firm only · rotate in Admin`.

---

## 6. Admin

Three panes: nav 248px · content flexible · compliance summary 300px.

**Left — nav** (grouped): **Firm** Users & roles (6) · Client access (ACL) · Devices (3) · Doc types (41) · Folder templates — **Intake** Print capture & parsers · Scan profiles — **Compliance** Audit trail · Retention classes (6) · Disposition (`38 eligible` warn) · Legal holds (1) · §7216 consents (4) · Safeguards appendix — **Security** Keys & recovery · Integrity sweep · Trash (12).

**Main — Audit trail** (the mocked page; other nav items follow the same header + filters + table pattern)
- Header: title 16px + muted `Hash-chained, append-only · also on disk at _Vibe\audit\2025-10.jsonl` · right `Export CSV` · `Export PDF` · primary **Verify chain now**.
- Green banner: `✓ Chain verified · 148,212 rows · Oct 29, 6:00 AM (weekly schedule) · next Nov 5` (red variant on failure).
- Filters grid: User · Client · Action · From · To · Document (name or UUID).
- Table: Seq (mono muted) · Time (mono) · User · Device · Action pill (file ok-green · redact red · export/conflict warn · sync_* info-blue · ocr_complete/checkout/login/integrity_sweep muted) · Document (mono) · Detail (free text; examples: `3 SSN boxes · verify: 0 matches remain · → (redacted).pdf`, `destination tagged third_party (lender) · §7216 consent #C-0412 active · allowed`, `Windows SID auto-login · TOTP for admin`) · Hash short.
- Footer row: `Showing 11 of 1,204 for Anderson Farms LLC since Oct 1` · right `Rows: {seq, ts, user, device, action, doc, detail, prev_hash, hash}`.

**Right — Compliance at a glance**: bordered rows — Catalog encryption (`SQLCipher · DPAPI` ok pill) · Last integrity sweep (`Oct 28 · 1 drift`) · Disposition (`38 eligible · review` link) · Legal hold (client pill, warn) · §7216 consents expiring 90d (`1`) · Last B2 push. Primary **Export Safeguards appendix (PDF)** (40px) + explainer. Section `Admin actions need TOTP`: stale lock override · empty trash · export firm cabinet key · user & role changes.

**Footer**: `vibe-files 0.9.0 · portable · D:\VibeFiles\` · `Catalog schema v1 · layout v1` · right `License: Kisaes LLC · internal use`.

---

## Cross-cutting behaviors

- **Every batch mutation shows preview → confirm → undo.** Confirm buttons state the count and the word "transaction" where the mock does.
- **Counts everywhere**: tree folders, queue sections, facets, nav items. Keep them live from the catalog, not cached at paint time.
- **PII masking in UI**: TINs always `•••-••-1234`; EINs `43-••••912` outside the Viewer canvas. The Viewer shows the page as-is because the user is looking at the document.
- **Mode awareness**: header pill, Search footer, and Sync devices all reflect `local / lan-writer / lan-reader / remote`. Remote mode disables Browse actions that need the full tree (bulk rename, integrity sweep) with a tooltip.
- **Empty states**: tree with no clients → "Create your first client or run the import wizard"; Intake with nothing pending → the drop zone fills the main pane; Search with no index yet → "OCR backlog running, N pages to go".
- **Keyboard**: `Ctrl+F` focuses the tab's search box; `Enter` on a grid row opens; `Space` toggles selection; `Ctrl+Z` undo last transaction; `Ctrl+Enter` confirms intake.
- **Density**: this is a work tool; rows are 32px, not 44px. Only the tab bar, auto-find list rows, and primary action buttons use 40–44px targets.

## egui mapping notes

- Panes = `SidePanel::left/right` + `CentralPanel`; tree = `CollapsingHeader` with custom row painting for counts and selection; grid = `egui_extras::TableBuilder` with `striped(false)`, sticky header, `Column::auto()` for pills and `remainder()` for Name.
- Pills, dots, chips, and segmented controls are small custom widgets in `vf-app::widgets`; implement once, reuse everywhere.
- Fonts: bundle IBM Plex Sans/Mono via `egui::FontDefinitions`; fall back to Segoe UI if the firm overrides.
- Viewer canvas: pdfium renders page tiles to textures; annotations drawn as egui shapes over the texture in page coordinates; commit to real PDF annotations on save.
- Long tables virtualize rows (`TableBuilder::body().rows()`), never paint 50k rows.
- Repaint on job completion via `ctx.request_repaint()` from worker threads; no polling timers faster than 250 ms.
