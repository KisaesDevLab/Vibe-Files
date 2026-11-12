# UI_SPEC.md — Vibe Filer Desktop

The authoritative UI reference. Derived from the approved mockup "Vibe Filer Desktop UI"
(11 screens, 1200×800). The mockup HTML files are checked in under `docs/mockups/` when
available; this document is the spec Claude Code builds from either way. Where PHASES.md and
this file disagree on layout or labels, this file wins. Where they disagree on behavior,
PHASES.md wins and the mismatch goes in QUESTIONS.md.

---

## 1. Visual system

| Token | Value |
|---|---|
| Window | 1200×800 default, min 1024×700, remembered |
| Font | IBM Plex Sans 13px body; IBM Plex Mono 12px for paths, filenames, IDs, patterns. Bundle both (OFL) in `assets/fonts/`. |
| Paper | `#F3F4F6` app background; cards `#FFFFFF` with 1px `#D7DBE0` border, 6px radius |
| Text | `#1B1F24`; muted `#5B6470`; labels 11px uppercase 0.04em `#5B6470` |
| Accent | `#0F6B8E` (active tab underline, primary button, badges, links) |
| Pills (20px, 11px bold, 3px radius) | ok `#DFF2E6/#166A3F` · warn `#FCEBC5/#7A4B00` · err `#F9E0E0/#9B2226` · info `#E1ECF8/#1F4D7A` · neutral `#EDF0F3/#4A525C` |
| Row tints | needs-attention rows `#FFFBF0`; dangling/duplicate rows `#FFF5F5`; copy sub-rows `#F7F9FB` with dashed bottom border |
| Buttons | 28px, 1px `#C9CED5`, 4px radius; `.primary` filled accent; `.sm` 22px |
| Fields | 26px, same border; invalid/attention border `#C98A1B` |
| Segmented control | Overwrite / Keep both / Skip, selected filled accent |

egui mapping: one `Visuals` with these colors; a `widgets::pill(kind, text)` helper; `egui_extras::TableBuilder` for every table; drag handles via `egui_dnd`.

## 2. Chrome (every screen)

**Header (44px, white, bottom border):** wordmark "Vibe Filer" + muted "Desktop" · tab nav
`Preview · Setup · Rules · Import · History · Conflicts · Relationships` with 2px accent underline on the
active tab · badges: Preview = pending queue count, Import = draft zips, Conflicts = unresolved count in amber
`#C98A1B` · right: "Profile" + active rules profile dropdown.

**Footer status bar (28px, 11px):** `Root <path>` · `N inboxes watched` · `N clients · N relationships` ·
pill `pdfium loaded` (err pill `pdfium missing` when not bound) · flexible gap · a context slot per screen
(Preview: "Undo available: batch 10:42 AM"; Setup: `data: <path>`; Rules: "rules.json saved HH:MM";
Import: `data\tmp · 61 MB`; Relationships: "relationships.json saved"; Split: "thumbnail cache 12 / 50";
Conflicts: "folders.json · N entries"; History: "ops.jsonl · N lines") · version.

Modals (Combine, Commit, CSV wizard) dim the screen behind with `rgba(27,31,36,.35)` and center a white
card with 8px radius and a drop shadow.

## 3. Preview (Main) — P05, P06, P07a, P07b, P08, P09

Toolbar: `Queue · N pending · N inboxes + N zip` · right side: **Partner** (V38, when clients have partners: only that partner's rows are listed and filed) · **Batch year** field · **On collision**
dropdown (Ask per file / Keep both / Overwrite / Skip) · **Keep all original names** switch · divider · `Browse…` · `Combine` (tooltip
"Select 2+ rows with the same client"; disabled unless the selection shares one resolved client) · `Rescan root`.

Table columns: ☐ · File · Client · Rule · Year · Destination · Status.

- **File**: mono filename; a 12-page scan shows `· 12 pages`; zip-sourced rows show a neutral `zip` pill with the original
  entry path in the tooltip; rows with copies show a ▾ collapse toggle.
- **Client**: name + muted mono ID. Unresolved: muted "No ID in filename" and a small button with the best fuzzy
  candidate and its score pill (`0.91`); clicking opens the picker. "No ID · no name match" when nothing.
- **Rule**: rule name; `sniffed` neutral pill when the first-page text sniff chose it; `Catch-all` for fallback.
- **Year**: value, or an inline field with amber border when missing.
- **Destination**: mono `…\<rel path>\<bold filename>`; a small "keep original name" toggle at the right of the cell swaps the templated name for the inbox name. Auto-resolved fuzzy rows show an info pill `fuzzy 0.98` in the Client cell. Needs-year rows show "Needs a year — [use batch year 2025 for all missing]"
  plus any related-client info (`no related clients` info pill + "Add relationship…" link). Needs-client rows say
  "Pick a client to compute". Multi-client scans say "Looks like several clients — [Split…]".
  Collision shows red mono "File already exists" and the segmented control inline.
- Copy sub-rows from `RelatedByName` show `not related` (info) with an "Add relationship…" link when the shareholder was found in the master list but not in the entity's relationships.
- **Status**: pills — `Ready` · `Needs year` · `Needs client` · `Collision` · plus modifiers `+N copies` (info),
  `Will create` (info), `Cross-drive` (info), `no PDF tools` (neutral, tooltip "Not a PDF: no text sniff, combine or split").

**Copy sub-rows** (P07a): indented under the primary, "copy · <role>" with a branch icon, the copy client, muted
"K-1 copy", year, destination, and an `×` "Remove this copy for this batch". Last copy row gets a solid bottom border.
A `+ Copy to…` link appears on the last row (adds a Manual copy for this batch).

**Unsupported footer** inside the table card: collapsed line `Unsupported (2) — a.docx, b.jpg · inbox drops are PDF only; zips go to Import`.

**Bottom bar:** counts `5 ready · 3 copies · 2 need a client · 1 needs a year · 1 collision resolved` · hint
"Resolve N rows to enable commit · Enter commits · Del skips row" · primary `Commit 8 files + 3 copies` (disabled until zero unresolved).

## 4. Commit modal — P06, P07

Title `Commit 8 files and 3 copies?` · subtitle `Profile Tax · batch year 2025 · nothing has been written yet`.
Five count tiles: moves (same volume) · copy·verify·delete · copies to related clients · folder trees to create ·
overwrites (with "· N kept both"). Then one line per notable category with a pill and detail:
`Copies first` (names + "each written, SHA-256 verified, then the primary moves; a failed copy rolls back and leaves the source in place") ·
`Will create` (folders + "+ template trees + sentinels") · `Cross-drive` (source → root, N files) · `Kept both` (resulting path) ·
`From zip` (N files from X.zip · temp folder removed once every entry has left the queue) · `Skip` (N rows).
Footer: "Every step is written to ops.jsonl. Undo is available until the next batch." · Cancel · primary Commit.

## 5. Setup — P01, P02, P03

Two-column card grid. A full-width amber banner at top when any inbox is on a different volume:
"Inbox on a different volume than root. Files from C:\Scans\Inbox will be copied, SHA-256 verified, then deleted from the inbox."

- **Client root**: path (mono) with `writable` ok pill · `Change…` · line "1,131 folders bound · 4 need attention in Conflicts · last scan today 9:12 AM" · `Rescan root` · `Open data folder`.
- **Inboxes** ("watched while the app is open · zips go to Import"): one row per inbox, cross-drive ones get an amber border and `cross-drive` pill, `Remove` per row · `Add inbox…` · note "An inbox cannot be inside the client root (except in a top-level _ . or ~ folder)."
- **Client master list**: CSV path · `Choose CSV…` · stats "1,284 clients · imported <date> · 3 rows skipped (blank ID) · 1 relationship now dangling" · Columns pills `ID → ClientID`, `Name → ClientName`, info pill `+ Entity, Partner, Email as tokens` · `Refresh clients` · `Re-map columns…` · `Stop using the CSV` (V37: the list stays, no CSV read again).
- **Filename parsing**: ID pattern (mono field + `valid regex` / `invalid` pill) · Convention line `ClientName_ID_rest.pdf` with ID highlighted · Fuzzy thresholds: *Suggest at* (default 0.85) and *Auto-resolve at* (default 0.97), two fields with validation auto ≥ suggest · note "Between the two a name match is suggested and needs a click; at or above auto-resolve it files with a `fuzzy` pill."
- **Folder template** (full width): header note "created on confirm, year folders only when a route needs them" · `Add child` `Rename` `Delete` `Reset to default` · left: tree with folder icons, tokens highlighted; right: "Tokens available" pills (core tokens neutral, CSV tokens info), copy-token note, sanitizer note.

## 6. CSV wizard modal — P02

860px. Title `Map CSV columns` · subtitle path · rows · columns · "header row detected".
Left: Required mapping — Client ID dropdown with `1,284 unique` ok pill; Client name dropdown with `3 blank → skipped` warn pill.
Optional — Partner dropdown (`(none)` or any column, e.g. Preparer): its values become `{Partner}` whatever the header (V35); Setup's Columns line then shows `Partner → Preparer`.
"Keep as template tokens" checkboxes per remaining column with `→ {Token}` preview; note about token naming.
Right: first 10 rows table; mapped ID column tinted, name column tinted, token columns tinted; a blank-ID row tinted red.
Footer: "Mapping is saved; later edits to the CSV re-import silently on launch." · Cancel · primary `Import N clients`.
Duplicate IDs block the import and are listed in place of the preview.

## 7. Rules — P04, P07a

Left (400px): `Profile: Tax · first match wins` · `New` `Rename` `Delete`. List rows: drag handle · enabled switch ·
name (with `+N copy` info pill when copies exist) · mono pattern. Disabled rules dimmed. Catch-all pinned at the
bottom with a lock icon, `.*`, and `always last` pill. Under the list: `Add rule` `Duplicate` "Drag the handle to reorder."

Right: **Edit rule** card — Name · Pattern (regex on rest, `valid`/`invalid` pill) · Primary destination "(moved)" · Filename template ·
checkboxes Requires year / Enabled. **Copies** section: note "real byte copies, written and hash-verified before the primary move" ·
`+ Add copy` · per copy a 5-column grid: Resolver dropdown (Related by name · group / Related · role / Capture · group / Manual · pick at review) ·
Destination · Filename · On split (Segment / Whole) · `×`; Related by name shows a Group field and the note "matched against this client's relationships first, then the master list"; Related shows a Roles row of pills with `edit…` and
"blank = every child of the primary client"; Capture shows a Group field. Tokens row (core + CSV, then "copies also:" Copy.* pills).
`Save` `Revert`.

**Test a filename** card — field + `Test` · result grid: Parsed (name/id/rest/ext/year) · Matched rule (+ "(Nth tried)") ·
Primary path · Copies ("3 related · partner ×2, member ×1 → first path +2 more").

## 8. Import — P07b

Left (640px): `Draft import · arrived in <inbox> today 4:51 PM` · `Browse for zip…`.
**Zip card**: name, "48 entries · 212 MB uncompressed · 61 MB on disk", `pre-scan ok` pill. Key/value rows:
Top level (summary of first-level folders/files) · Dropped (`3 junk` + names) · Blocked (`1 executable` + path · "never extracted") ·
Unsafe path (`1 entry` + path · "rejected") · Nested zips ("1 · imported as a file, not expanded") ·
Guards ("≤ 2,000 entries · ≤ 100 MB per entry · ≤ 1 GB total · not encrypted"). Encrypted or over-limit zips show an err pill and no mode cards.
**Mode cards** (radio): "Unpack to a client folder — Keeps the zip's folder structure. Any file type. You pick the client." /
"Send contents to the queue — Extracts flat, then the normal rules flow in Preview. N files."
**Unpack destination** (Mode A only): Client field + `1877 · exact` pill + "typed, never guessed from the zip name" ·
Subfolder field with datalist of existing subfolders + "blank = client root" · "Lands in" full path.
**Collisions** card ("checked before anything extracts"): batch default dropdown · table path · "212 KB exists" · segmented control.
Action row: note about `.tmp` + hash + undo copy under `data\tmp\undo\` · `Discard draft` · primary `Unpack N files` / `Queue N files`.

Right: **Recent imports** table (Zip + date + client ID / Mode / Result pills such as `118 written` `2 skipped` `encrypted · rejected` `undone`),
draft row tinted; **detail card** for the selected import: Batch id/time · Destination · Entries summary · Source zip status ·
`Open folder` · `Show in History`.

## 9. Relationships — P07a

Header: `Relationships · N records · N dangling after the <date> CSV refresh · saved on every edit` · `Import CSV…` `Export CSV`.
Left (340px) card: search field ("ID or fuzzy name · N matches"), result rows (name, mono ID, `N related` pill, fuzzy score),
selected panel: name, `ID 4472`, CSV extras line, "Folder bound · <path>" or "No folder".
Right: two cards — **Owners / related of this client** ("<name> is the parent" · "K-1 copies go to these") and
**This client is related to** ("<name> is the child"). Each: table Client / Role dropdown / Note field / `×`;
dangling rows tinted red with `dangling` pill, `Re-link…` `Remove`; last row is the add row (client picker, role, note, "Enter").
Footnote: "Stored once, parent → child. 'spouse' is a role, not mirrored. Both IDs must exist in the master list; a refresh that drops one flags the row instead of deleting it."

## 10. Split — P09

Header row: source filename, page count, "auto-proposed from text layer" or "no text layer — manual cuts".
**Thumbnail strip**: horizontally scrollable page thumbnails; between pages a thin cut toggle (accent bar when on); page label
shows the page number and, when found, the matched ID in accent or "name".
Below, two columns: **Page preview** (selected page rendered by pdfium, "rendered by pdfium · cached", footer "Matched on this page: 4472 · "Riverbend Holdings" (line 4)") and
**Segments** list ("each becomes its own row in Preview; the original is removed after all N commit"): per segment a card with
`Segment N · pages a–b` · match pill (`ID 4472 on page 1` ok / `name only · 0.88` warn) · Client dropdown (amber when unresolved, best candidate first) ·
Rule (+ `sniffed`) · Filename field · Copies summary ("3 related · on split: Segment" + who gets them, or "none for this rule").
Footer note: "Writes N PDFs to data\tmp\; a rule with 'Whole' copies keeps the original as that copy instead of deleting it." · Cancel · primary `Apply split · N rows`.

## 11. Combine modal — P08

620px. `Combine 3 PDFs` · "All selected rows resolve to <client> <id> · 41 pages total". Ordered list with drag handles, index, mono name,
page count, `Remove`. Fields: Output name (defaults from the first file's proposal) · Rule → destination · After commit note
("The originals are deleted from the inbox only once the merged file lands and verifies. Undo restores them."). Footer:
"Merged with lopdf into data\tmp\" · Cancel · primary `Combine · replaces N rows`.

## 12. Conflicts — P03

Header: `Folders that are not bound to a client · from the scan of <root> <time>` · `Show ignored (N)` checkbox · `Rescan root`.
Four stat tiles: Bound (sentinel matches master) · Unbound (ID or name found) · Orphan (sentinel ID not in master) · Duplicate (same sentinel in two folders).
Table Status / Folder / Candidate client / Action:
- `Unbound · named` — folder + "ID 4480 found in folder name · no _Vibe\client.json" · candidate with `ID match` pill · `Bind · write sentinel` `Ignore`.
- `Unbound · fuzzy` — candidate dropdown with scores and "Other…" · `Bind` `Ignore`.
- `Orphan` — "Sentinel says client_id 5120 · not in current CSV · N relationship(s) also dangling" · `Rebind to…` `Ignore`.
- `Duplicate` — both folders listed · "routes and copies to <id> are paused until resolved" · `Keep first` `Keep second` `Ignore`.
Footnote: "Bind writes _Vibe\client.json (T&B-compatible v1: client_id, firm_id, tax_software_id, display_name_at_creation, created_at). Nothing is moved or renamed here."

## 13. History — P07

**Last batch card**: time · short batch id · summary ("5 moves, 1 copy-verify, 3 copies, 1 folder created, 1 zip routed · 6.4 MB") ·
undo preflight on the right ("All primaries present at recorded size · 1 copy modified since filed will be left in place and reported", or the reason undo is disabled) ·
`Undo last batch  Ctrl+Z`.
Filter row: client · from/to date · batch dropdown · result dropdown · op dropdown (all op values) · "N records · newest first".
Table Time / Op / Client / Source → Destination / Result / action. Batch header rows span the table ("Batch b7e1 · Today 10:42 AM · 12 records").
Op pills: neutral for move/mkdir/split/merge/zip_import/zip_entry/undo/skip, info for copy/copy_verify_delete, err for rollback.
Copy rows show "of <primary id>", the sha256 prefix, `kept both` or `modified since` pills. Zip-routed rows show the temp path with the original entry in parentheses.
`Open folder` per row; zip_import rows get `Show import`.

## 14. Behaviors the mockup adds to PHASES.md

These are now in scope; the owning phase is noted.

| Behavior | Phase |
|---|---|
| Footer status bar with per-screen context slot | P00 |
| Tab badges (Preview pending, Import drafts, Conflicts amber) | P00 / P05 / P03 / P07b |
| Cross-drive banner on Setup and `cross-drive` pill per inbox | P01 |
| Client master stats line includes "N relationship(s) now dangling" | P07a |
| Conflicts: stat tiles, `Show ignored` toggle, Orphan `Rebind to…`, Duplicate `Keep first / Keep second` | P03 |
| **Duplicate sentinel pauses routing and copies to that client** until resolved (Preview rows get a `Paused · duplicate folder` warn status) | P03, P06 |
| Rules: `Duplicate` rule button; `(Nth tried)` in the test panel; copies resolved in the test panel | P04, P07a |
| Preview: `no PDF tools` pill on non-PDF rows; Unsupported collapsed footer line; keyboard hint text | P06, P07b |
| Commit modal exact content (§4) | P06 |
| History: last-batch card with undo preflight; batch header rows; `modified since` pill | P07 |
| Import: `Discard draft`; recent-imports result pills; detail card | P07b |
| Relationships: selected-client panel shows CSV extras and folder binding | P07a |
| Split: matched ID in page labels; "Matched on this page" line under the preview | P09 |
