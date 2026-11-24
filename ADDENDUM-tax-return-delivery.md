# Addendum — Tax return delivery, advisory packages and the balance owed card (Vibe Portal and Vibe Files)

Status: draft for the owner's review, 2026-11-24. Nothing here is built. One document for the
whole feature across both repos: sections 1–11 are the design and the Vibe Portal work, sections
12–13 the Vibe Files work, section 14 the order to build both in, section 15 the UI specification, section 16 the separate balance owed card. Keep the same file in each
repo (as `ADDENDUM-02-tax-return-delivery.md` in Vibe-Portal and
`ADDENDUM-tax-return-delivery.md` in Vibe-Files) so neither build reads half the plan.

Decisions are the owner's Q&A answers of 2026-11-24 (T1–T30 below). When this is accepted, copy
them into each repo's `QUESTIONS.md` under its next free numbers (Vibe Portal P, Vibe Files Q)
and give the phases their P and PA numbers.

New scope goes here, not into `BUILD_PLAN.md` or the core plan files. This reverses two lines of
Vibe Portal's `BUILD_PLAN.md` §8: "Billing and payments" and "engagement letters and
e-signature".

Screens: design canvas "Tax return delivery — Vibe Portal & Vibe Files"
(https://claude.ai/artifact/JL9nXCC2nqEK1WybKGPk88). Where a screen and this text differ, this
text wins; section 15.7 says which screens predate T17–T30.

Letter text: the Advantage and Concierge package addenda are drafted in "Advantage and Concierge
package addenda — draft" (https://claude.ai/artifact/WxUV9PYsuC9qSppC4nk5RS). They become
letter templates in Vibe Files (section 12) once the owner's attorney has reviewed them.

## 1. What it is

A card at the top of the client's home page, "Your 2025 tax return", that walks the client
through up to five steps. Staff build each delivery in Vibe Files. The portal keeps it, runs it,
logs it, and sends a PDF of each finished step back to the client's folder.

| # | Step | The client | Done when |
|---|---|---|---|
| 1 | Letters | Reads each letter, draws a signature, agrees, signs. A second signer follows on the same device | Every required signer signed every letter |
| 2 | Tax return | Views or downloads the PDF (an ordinary shared file) | First opened or downloaded |
| 3 | Package and add-ons | Picks a package and any add-ons, signs the selection and the package addendum | Signed |
| 4 | Fee | Pays the total on Stripe's page | Payment started (bank) or paid (card), or staff mark it paid |
| 5 | Form 8879 | Opens UltraTax eSignature in a new tab, signs there, presses "I've signed it" | Staff confirm |

A delivery may have any subset of the steps (T6). Step 3 exists only when the fee mode has
something to choose (§5). The bar and "N of M" count only the steps present.

## 2. Decisions

| # | Topic | Decision |
|---|---|---|
| T1 | Locking | Staff choose per delivery what must finish before the return and the 8879 link open; a firm default sets the starting switches |
| T2 | Letter e-sign | Built in, with a drawn signature |
| T3 | Joint signers, letters | Both spouses on one device: the second signs after a code sent to them |
| T4 | Letter source | Templates in Vibe Files, a set per return type (not the UltraTax PDFs) |
| T5 | Template editor | Inside Vibe Files, with merge fields and signature blocks |
| T6 | Card timing | Staff start a card with any subset of steps and add the rest later |
| T7 | Where staff work | Created in Vibe Files; the staff page follows (view, remind, mark paid, confirm 8879) |
| T8 | Fee | Paid on Stripe Checkout through the firm's own Stripe account |
| T9 | Methods | A firm setting: card, bank account, and a card fee, each on or off |
| T10 | Fee modes | One amount; base fee plus add-ons; or packages plus add-ons |
| T11 | Letter and fee options | The engagement letter lists the options; the signed selection completes it |
| T12 | Pricing plans | Per return type in Vibe Files Admin; staff untick options and may override any price per client |
| T13 | After payment | Locked. Add-ons or upgrades later are handled outside the portal |
| T14 | 8879 link | UltraTax eSignature, one link per client |
| T15 | 8879 done | The client says "I've signed it", then staff confirm |
| T16 | Packages | Tax Return Only, Advantage, Concierge (names and lines from the plan), one marked Recommended |
| T17 | Package order | After the return is reviewed, before the fee: letters, return, package, fee, 8879 |
| T18 | Package price | A percent of the client's return fee with a minimum amount; staff can override the result |
| T19 | Package signer | Whoever chooses signs, for both spouses on a joint return |
| T20 | Package addendum | Advantage and Concierge each need a signed addendum to the engagement letter; Tax Return Only needs none |
| T21 | Return types | Letters, packages and addenda for 1040, 1120, 1120-S and 1065 |
| T22 | Entity signer | One person staff pick per delivery; their title is recorded |
| T23 | Step PDFs | Each step files one PDF: the step's document, then a separate page logging that step's actions |
| T24 | Where filed | `Income Tax\{Year}\Delivery` in the client's folder |
| T25 | Return step PDF | A record page (file name, pages, SHA-256), not a second copy of the return |
| T26 | Bank payments | Steps locked behind the fee open when the payment is started; a later failure tells staff and shows the fee unpaid again |
| T27 | Revised return | The card shows "Updated" and reopens the 8879 step; letters, package and fee stand |
| T28 | Client boxes | Add-ons start unticked and no package is preselected; "Recommended" is a tag only |
| T29 | Plan changes | A plan edit affects new deliveries only; a delivery keeps the prices it was sent with |
| T30 | Switch | Off until an admin turns it on, as To-do lists (P109) |

## 3. Client pages

- **Card** (`pages/Delivery.tsx`, mounted in `Home.tsx` above `ToDo`): title, progress, one row
  per step. Shown for the active client when a delivery is open, or finished within
  `firm.delivery_keep_days` (default 30); older ones under "Past tax returns" in Files.
- **Step states**

| State | Shown as | Meaning |
|---|---|---|
| Coming | Grey, no button | Staff have not added this step's content |
| Locked | Padlock, "Opens after step N" | Its conditions are not met |
| Open | Blue number; "Next" on the first open step | The client can act |
| Waiting | Amber | "Waiting for the firm to confirm" (step 5) or "Payment started" (step 4) |
| Done | Green tick, date, who | Links to signed copies or the receipt stay |

- **Locks** apply to step 2 (letters signed) and step 5 (letters signed, fee paid), each a switch
  per delivery (T1). Step 3 opens with step 2; step 4 opens when step 3 is signed. A locked
  return is hidden everywhere: card, Files list, preview and download return 404, as a file not
  shown to the person does today.
- **Sign** (`/delivery/:id/sign`): the letter as a PDF preview; a signature pad (pointer events,
  kept as PNG); "I have read this letter and agree to sign it electronically"; **Sign as
  <name>**. Then the next letter, then the next signer.
- **Second signer** (T3): after the first person signs, "<Name>, your turn": a 6-digit code goes
  to that signer's own email or mobile (from the delivery, §6), 10 minutes, 5 tries, 3 sends.
  The code proves the second person; their signature is logged under their name with
  `via: same_device` and the first person's session id. They need no portal login.
- **Package** (`/delivery/:id/package`): packages side by side (stacked pick list on a phone)
  with the plan's service lines; add-ons below; a summary with the total; the addendum for the
  chosen package to read; one drawn signature (T19); **Sign and continue to payment**. The
  selection can change until signed.
- **Fee**: the lines and total, **Pay $X** → Stripe Checkout → back to the card. After payment:
  the lines, the method, **Receipt**.
- **8879**: **Open eSignature** (the per-client link, new tab, `rel="noopener"`), then **I've
  signed it** → Waiting until staff confirm (T15).
- Accessibility and layout follow P71/P72: real controls, 44 px targets, works at 390 px, light
  and dark.

## 4. E-signature (steps 1 and 3)

- Vibe Files makes each letter PDF from its template and sends it with the **signature blocks**:
  page, rectangle, signer role (`taxpayer`, `spouse`, `officer`), and a date rectangle.
- On each signature the portal appends to `delivery_event` (append-only by trigger, as
  `request_response`): time, `clientIp()`, browser, the person's name/email/phone as they were,
  the session hash, the letter's SHA-256 before signing, the signature image's SHA-256, and
  `via` (`own_login` or `same_device`).
- When the last signer has signed, the portal builds the signed PDF with pdf-lib: signature
  images and dates placed in their blocks, then flattened. The signed PDF is shared to the
  client's users (Files list) and goes into the step PDF (§7).
- The checkbox wording, the signer's intent (the Sign button names them), the tie between
  signature and document (hashes), and a copy to each signer are the record. The owner's
  attorney confirms this meets Missouri's electronic-signature law and the federal E-SIGN Act
  for these letters before release.
- §7216 consents are their own letters with their own templates. The portal treats them as any
  letter; the required wording and format are the firm's template text (owner to confirm against
  Treas. Reg. §301.7216-3 and Rev. Proc. 2013-14).
- Form 8879 is **not** signed here (T14). Remote 8879 signatures need the identity check that
  UltraTax eSignature does.

## 5. Fee, packages and add-ons (steps 3 and 4)

- A delivery carries a **priced snapshot** from Vibe Files (T29): mode, lines, and prices
  already worked out per client. The portal never computes a price from a plan.

| Mode | Step 3 | Step 4 total |
|---|---|---|
| `amount` | none | the amount |
| `addons` | tick add-ons, sign | base fee + ticked add-ons |
| `packages` | pick one package, tick add-ons, sign | package price + ticked add-ons |

- `delivery_fee_line`: `kind` (`base`, `package`, `addon`), name, the text the client reads,
  price in cents, `recommended`, `selected`, order. A package line carries its service lines
  (group, bullets) for the comparison.
- The signed selection is a `delivery_event` and a one-page selection sheet in the step 3 PDF.
  For Advantage and Concierge the chosen package's addendum PDF is signed in the same act (T20).
- **Stripe** (firm's own account; keys in Settings › Payments, sealed with the firm key, never
  sent back): the API creates a Checkout Session in `payment` mode with one line per selected
  fee line, prices from `delivery_fee_line` (never from the browser), the delivery id in
  metadata, and success/cancel URLs back to the card. A webhook endpoint
  (`POST /api/stripe/webhook`, raw body, signature checked, events de-duplicated by id) records
  completed, async succeeded and async failed. No card or bank data is stored.
- T26: a bank payment sets `payment.state = started` and satisfies "fee is paid" for locks; a
  failure sets `failed`, tells the assigned staff, and step 4 shows unpaid with **Pay** again.
  An 8879 link already opened stays open; staff decide what to do.
- **Mark paid** (staff, with a note such as the check number) completes step 4 without Stripe.
- Card fee (T9): if the firm turns it on, it is one more line, shown before Pay. The owner
  confirms card-network and state rules before turning it on.
- Verify every Stripe call and event name against Stripe's current documentation at build time;
  none is assumed from memory.

## 6. Data model (migration `00NN_delivery.sql`)

| Table | Purpose |
|---|---|
| `delivery` | `id`, `client_id`, `tax_year`, `return_type`, `title`, `status` (open / complete / cancelled), `lock_return_letters`, `lock_8879_letters`, `lock_8879_fee`, `assigned_staff_id`, `remind_days`, created/sent/completed |
| `delivery_step` | `delivery_id`, `kind` (letters / return / package / fee / esign_8879), `present`, `state`, `completed_at`, `completed_by`, `pdf_upload_id` |
| `delivery_letter` | `id`, `delivery_id`, `step` (letters / package), `name`, `doc_type`, `blob_key`, `dek_wrapped`, `sha256`, `blocks` (json), `package_line_id` (addendum), `signed_blob_key`, `signed_sha256` |
| `delivery_signer` | `delivery_id`, `role`, `name`, `title`, `identity_id` (null for a same-device signer), `email`, `phone`, `order` |
| `delivery_signature` | `letter_id`, `signer_id`, `image_blob_key`, `image_sha256`, `signed_at`, `via` |
| `delivery_code` | Second-signer codes: hash, expiry, tries, sends |
| `delivery_fee_line` | As §5 |
| `delivery_payment` | `delivery_id`, `provider` (stripe / manual), `session_id`, `state`, `amount_cents`, `method`, `note`, times |
| `delivery_event` | Append-only: every action in §7's log pages, with time, address, browser, person or staff, PC name, detail json; no foreign keys, so it reads the same after a purge |
| `files` | The return PDF and signed letters are ordinary rows; `delivery_id` marks them |
| `firm` | `deliveries_enabled`, lock defaults, `delivery_keep_days`, Stripe settings (sealed), methods, card fee |

One open delivery per client, tax year and return type. An entity and its owner are different
clients, so each has its own.

## 7. Step PDFs and log pages (T23–T25)

When a step completes the portal builds one PDF (pdf-lib, DejaVu, as the to-do report) and
queues it for Vibe Files as an upload tagged `delivery_step`, which Intake files in
`Income Tax\{Year}\Delivery` (section 12.5).

| Step | File name | Pages before the log | Log lines |
|---|---|---|---|
| 1 | `{Year} Delivery 1 - Letters.pdf` | Each signed letter | Sent, opened, each signature (who, role, via), code sent and accepted |
| 2 | `{Year} Delivery 2 - Tax return.pdf` | Record page: file name, pages, size, SHA-256 | Shared, unlocked, each view and download |
| 3 | `{Year} Delivery 3 - Package selection.pdf` | Selection sheet; the signed package addendum if any | Options offered with prices, each change, signature |
| 4 | `{Year} Delivery 4 - Fee.pdf` | Receipt: lines, total, method | Checkout started, paid / started / failed / cleared, or marked paid by staff |
| 5 | `{Year} Delivery 5 - Form 8879.pdf` | Record page naming the link's host | Link opened, client said signed, staff confirmed |

- The log page is always a separate page, headed with the firm, client, tax year and step. Each
  line: date and time with the zone, the person (or staff name and PC), internet address,
  browser, what happened.
- A step that completes again (a letter re-sent, T27's reopened 8879, a failed bank payment
  retaken) files a new PDF with ` (2)`, ` (3)` before `.pdf`. Nothing is overwritten.
- `GET /deliveries/:id/report.pdf` gives the whole delivery on demand; it is not filed by itself.

## 8. APIs

**Firm API** (`/api/firm/v1`, firm key; `/status` `features` gains `deliveries`):

| Method | Path | Does |
|---|---|---|
| POST / GET / PATCH | `/deliveries`, `/deliveries/:id` | Create (any subset of steps), read with states and events, change while a step is not done |
| POST | `/deliveries/:id/letters` | Upload a letter PDF with `blocks`, `doc_type`, step |
| PUT | `/deliveries/:id/return` | Point at a file already on the portal; replacing it later is T27 |
| PUT | `/deliveries/:id/fee` | The priced snapshot (§5); refused once step 3 is signed |
| PUT | `/deliveries/:id/esign-link` | The UltraTax link (https only, as P76) |
| POST | `/deliveries/:id/confirm-8879`, `/mark-paid`, `/remind`, `/cancel` | Staff actions |
| GET | `/deliveries/:id/report.pdf` | The whole-delivery report |
| GET | `/uploads` | Step PDFs appear as uploads with `delivery: { id, step, year }` |

**Client API** (`/api/portal/deliveries`): list for the active client; letter preview; `sign`;
`second-signer/send-code`, `second-signer/verify`; `selection` (save), `selection/sign`;
`checkout` (returns the Stripe URL); `said-signed`.

**Staff API**: read, remind, mark paid, confirm 8879, cancel; Settings › Tax return delivery
(switch, lock defaults, keep days) and Settings › Payments (Stripe keys, methods, card fee,
**Test**), admins only.

## 9. Notices and reminders

As to-do lists (P109): email and text when a delivery is sent and when a step opens; **Remind**
by hand, at most 3 a day; automatic every N days while a step waits on the client. Notices carry
the title and the step name only, never amounts or letter text. The assigned staff member is
told when a step completes, when a client says "signed", and of every payment event on the fee
step: paid, bank payment started, and failed (B16; the email names the client, amount, method
and time). With nobody assigned, payment notices go to the firm's "Payment notices go to"
person (Settings › Payments).

## 10. Security notes

- Every delivery route checks the person's access to the client, and for a step, that it is
  open and not locked, on the server.
- Signature images, letters and signed PDFs are stored as every blob (per-file DEK, firm key).
- The webhook route is the only unauthenticated write; it accepts nothing without a valid
  Stripe signature and changes only the payment it names.
- The 8879 link is stored and shown as P76's client link is: https only, no control characters.
- CSP: add Stripe's Checkout host to `form-action`/navigation only as far as a redirect needs;
  no Stripe script is loaded on the portal (hosted page, T8).
- A security review of the new routes before release, as Phase 7 did.

## 11. Vibe Portal phases

Number them from the next free P when accepted. Each ends with `pnpm gate` green.

### D0 — Payments foundation (built first, with D7)
- [x] Settings › Payments: the firm's Stripe keys sealed with the firm key, methods, card fee,
  **Test**; `firm` columns
- [x] `POST /api/stripe/webhook`: raw body, signature checked, events de-duplicated by id,
  routed by metadata to a balance (D7) or, later, a delivery (D5)
- [x] CSP and redirect handling for Stripe's hosted page; no Stripe script on the portal
- **Gate:** Test succeeds with test keys and says what is wrong with bad ones; a forged webhook
  is refused; a replayed one changes nothing; keys never appear in logs, History or any response

### D1 — Deliveries, the card, return and 8879 steps
- [ ] Migration; `firm.deliveries_enabled`; firm API create/read/change/cancel; `/status` feature
- [ ] Card on Home; states; locks enforced on files routes; `said-signed`, `confirm-8879`
- [ ] `delivery_event` append-only trigger; events for every action
- **Gate:** a delivery with steps 2 and 5: the return is 404 while locked and listed once
  unlocked; said-signed shows Waiting; confirm completes; a second client's user sees nothing

### D2 — Step PDFs with log pages
- [ ] PDF builder (document pages + separate log page), names, ` (2)` on repeat
- [ ] Queued to `/uploads` with the `delivery` tag; whole-delivery report
- **Gate:** steps 2 and 5 each produce their PDF with the right log lines; a reopened step 5
  files ` (2)`; Vibe Files' e2e files both in `Income Tax\{Year}\Delivery`

### D3 — Letters e-sign
- [ ] Letter upload with blocks; sign page; signature pad; agreement; per-signer order
- [ ] Second signer on the same device (codes, limits); entity signer with title
- [ ] Signed PDF (placed, dated, flattened), shared to the client's users; step 1 PDF
- **Gate:** two letters, two signers on one device: wrong code refused, 6th try locked; signed
  PDF hashes match the events; a letter changed after one signature is refused

### D4 — Packages and add-ons
- [ ] Fee snapshot; package page (desktop and phone); add-ons; selection sheet
- [ ] Package addendum signed with the selection (T20); step 3 PDF
- **Gate:** the three modes; total equals the selected lines; `PUT /fee` refused after signing;
  nothing preselected (T28)

### D5 — Payment
- [ ] On D0's settings and webhook: the delivery's Checkout Session and receipt
- [ ] Bank started/failed/cleared (T26); mark paid; step 4 PDF
- **Gate:** against Stripe test mode: card paid; bank started opens step 5, then a failed event
  shows unpaid and tells staff; a replayed webhook changes nothing; a forged one is refused

### D6 — Reminders, staff page, revised return, review
- [ ] Notices and reminders; assigned staff; staff page views and actions
- [ ] T27 (Updated, step 5 reopens, new link needed); finished cards and "Past tax returns"
- [ ] Security review; load test with 500 clients each with a delivery; `docs/firm-setup.md`
- **Gate:** the owner's end-to-end walk with Vibe Files on a real phone over the tunnel

## 12. Vibe Files side

Vibe Files is where a delivery is made: it alone reaches the client folder, the templates and
the pricing plans. Everything below sits behind `PortalSettings.deliveries` (Admin › Portal,
every PC) and shows only when the portal's `/status` `features` has `deliveries`.

### 12.1 Deliver tax return window (`ui/delivery.rs`)

- Opened from the client panel (**Deliver tax return…**) and from a return PDF's right-click.
  One window per client, tax year and return type; reopening shows the same delivery.
- Top: tax year, return type (1040, 1120, 1120-S, 1065), card title.
- One section per step, each with a tick to include it (T6) and its state from the portal:

| Section | Staff set |
|---|---|
| 1 · Letters to sign | Template set for the return type; which letters; who signs (from the client's Portal contacts, PA-105: taxpayer and spouse, or one person with a title for an entity, T22); **Preview letters…** |
| 2 · Tax return PDF | The file, chosen from the client folder (default: the newest "Return copy — federal" for the year); "Opens after letters are signed" |
| 3 · Package and fee | Mode (one amount / base fee and add-ons / packages and add-ons); the pricing plan; the client's return fee; each package's price worked out and overridable; which add-ons are offered; **Mark paid…** |
| 4 · Form 8879 | The client's UltraTax eSignature link (**Paste**); "Opens after" letters signed / fee paid; **Confirm signed** |

- Right side: what has happened (from the portal's events), **Tell the client** (notices,
  remind every N days, **Remind now**), **Download report…**.
- Bottom: **Send to client** (first time) / **Save changes**, **Cancel delivery…**.
- A second signer needs their own email or mobile in Portal contacts; the window says so and
  links to **Edit contacts…** when it is missing.
- Replacing the return PDF on a sent delivery asks "Tell the client the return was updated and
  reopen the 8879 step?" (T27).

### 12.2 Letter templates (Admin › Letter templates, `ui/letter_templates.rs`)

- Sets per return type. Each letter: the name the client sees, the body, the doc type it is
  filed as, "on the firm's letterhead", "ticked by default in a new delivery".
- Editor: headings, paragraphs, bold, italic, lists, a simple table, **Insert field**, **Insert
  signature**. Stored as a small JSON document in the cabinet (`_Vibe/letters/…`), versioned;
  a delivery records the version it used.
- Fields: Client name, Client ID, Client address, Signer first names, Tax year, Next year,
  Return type, Covered returns, Fee, Fee options (the table of T11), Package name, Package fee,
  Date, Firm name, Preparer.
- Signature blocks: Taxpayer, Spouse (left out when there is none), Officer/Partner with title.
  The layout step records each block's page and rectangle for the portal (section 4).
- Paragraphs can be marked "only for" a return type or a package, so one addendum template
  serves 1040, 1120, 1120-S and 1065 (T21) and section 11 of the package addenda keeps only the
  paragraph that applies.
- New crate `vf-letters`: template model, merge, layout to PDF on letter-size pages with the
  firm letterhead, block rectangles out. The PDF writer is chosen at build (what `vf-pdf`
  already links first; no new native dependency without asking).
- Seeded templates, all editable and marked "Sample — review before use": engagement letter per
  return type (headings only, the firm pastes its own text), consent to disclose, consent to
  use, Advantage Package Addendum, Concierge Package Addendum (from the reviewed draft).

### 12.3 Pricing plans (Admin › Pricing plans, `ui/pricing_plans.rs`)

- A plan per return type and tax year: base fee name and starting price; packages; add-ons.
- **Packages** as a grid: columns are packages, rows are services; each cell holds the bullet
  lines the client compares. Per package: name, Recommended (at most one), and its price rule,
  **a percent of the client's return fee with a minimum amount** (T18). The first package is
  the return fee itself.
- Price worked out in cents: `return_fee + max(return_fee × percent, minimum)`, rounded half up
  to the cent. Sample plan: Advantage 10 % (minimum $127), Concierge 35 % (minimum $443); on a
  $1,265.68 return fee both minimums apply and give $1,392.68 and $1,708.68.
- **Add-ons**: name, what the client reads, price, "offered unless unticked".
- **Copy 2025 plans to 2026…**. A plan edit never changes a delivery already sent (T29): the
  delivery holds its own priced snapshot.
- Stored in the cabinet (`_Vibe/pricing/…`), audited by field name.

### 12.4 Package addenda

- Choosing Advantage or Concierge needs its addendum signed (T20). Vibe Files sends the addendum
  PDF for **every offered package that has one** with the delivery, each tied to its fee line,
  so the portal can show and sign the right one without a round trip.
- The addendum's signature block is one signer (T19): the person who chooses, "signing for
  <both names> on a joint return"; for an entity, the picked signer and title.

### 12.5 Filing what comes back

- Step PDFs arrive through the Portal inbox as uploads tagged `delivery` (section 7). The fetch
  writes an `ingest_hints.jsonl` hint as PA-103 did: origin `portal_delivery`, the client, the
  tax year, the step, and the target folder `Income Tax\{Year}\Delivery`.
- Intake files them there without asking, creating `Delivery` when missing, under the names in
  section 7. Doc types (new, seeded): Delivery — letters, Delivery — return record, Delivery —
  package selection, Delivery — fee receipt, Delivery — 8879 record; retention class "Returns &
  workpapers".
- A PDF for a client or year Intake cannot place stays in the Portal inbox with the hint shown,
  as any upload.

### 12.6 The rest

- `vf-portal::delivery`: types and calls for section 8's firm API; `Status.features`
  (`deliveries`); new error codes in words.
- Each delivery read is copied to `<client>/_Vibe/delivery/<id>.jsonl` (one line per changed
  version, as to-do lists), shown when the portal is out of reach.
- Browse: a small mark on a client with an open delivery; the client panel shows "Tax return
  delivery: 3 of 5 done".
- A list, **Deliveries**, of every open delivery with its step, days waiting and assigned staff;
  filters by step and staff. This is the season's worklist.
- Audit rows: `portal_delivery_create`, `_change`, `_send`, `_remind`, `_mark_paid`,
  `_confirm_8879`, `_cancel`, `_report`, `_copy`; `letter_template_save`, `_delete`;
  `pricing_plan_save`, `_delete`. Field names only, never amounts or contacts.
- Help: `portal-delivery`, `letter-templates`, `pricing-plans`.

## 13. Vibe Files phases

Number them from the next free PA when accepted. Each ends with the workspace tests green and
screenshots of the new windows.

### F1 — Deliveries: return and 8879 (with portal D1, D2)
- [ ] `vf-portal::delivery`; `PortalSettings.deliveries`; Deliver window with sections 2 and 4
- [ ] Hints and Intake filing into `Income Tax\{Year}\Delivery`; the five doc types
- [ ] Local copies; audit rows; client panel line; Deliveries list
- **Gate:** `scripts/portal-e2e.sh`: create, client opens the return, says signed, staff
  confirm; both step PDFs filed in the Delivery folder with their log pages

### F2 — Letter templates and signing (with portal D3)
- [ ] `vf-letters` (model, merge, layout, block rectangles); Admin › Letter templates
- [ ] "Only for" paragraphs; letterhead; seeded samples
- [ ] Deliver window section 1; signers from Portal contacts; Preview letters
- **Gate:** a 1040 joint letter and a 1065 letter laid out, their blocks land on the printed
  signature lines (checked on rendered pages); e2e: two signers, signed PDF filed

### F3 — Pricing plans, packages and add-ons (with portal D4, D5)
- [ ] Admin › Pricing plans (grid, percent with minimum, add-ons, copy to next year)
- [ ] Deliver window section 3: three modes, return fee, worked-out prices, overrides, snapshot
- [ ] Fee options field; package addendum templates sent per offered package
- [ ] Mark paid; payment states shown; failed bank payment notice on every PC's footer
- **Gate:** the sample plan gives $1,392.68 and $1,708.68 on $1,265.68; an override is kept; a
  plan edit leaves a sent delivery unchanged; e2e in Stripe test mode through step 4's PDF

### F4 — Finish
- [ ] Revised return (T27); cancel; reminders; help pages
- [ ] Owner's walk: a real 1040 joint client and one entity client, phone and desktop
- **Gate:** every step's PDF and log page in `Income Tax\{Year}\Delivery`; the whole-delivery
  report matches them

## 14. Build order across both repos

| Slice | Portal | Vibe Files | The firm can then |
|---|---|---|---|
| 1 | D0, D7 | F5 | Take a Stripe payment for a balance posted from Vibe Files, on a card that can remove itself (section 16) |
| 2 | D1, D2 | F1 | Deliver a return and the 8879 link on a card, with locks, and get the step PDFs filed |
| 3 | D3 | F2 | Have letters signed on the portal, both spouses on one device |
| 4 | D4, D5 | F3 | Offer packages and add-ons, and take the delivery fee |
| 5 | D6 | F4 | Run a season: reminders, revised returns, the worklist |

Owner 2026-11-24: the balance card first, with the Stripe setup; within the delivery, letters
before packages and payment.

Each slice is released on its own (portal image, then Vibe Files). A portal without a slice's
feature answers 404 for its routes and leaves it out of `/status` `features`; Vibe Files hides
what the portal does not offer, so either side may update first.

## 15. UI specification

The build cannot open the design canvas, so this section is the layout contract, as
`docs/UI_SPEC.md` is for Vibe Files. The canvas remains the owner's picture of it.

### 15.1 Portal: shared rules

- Built from what the portal has: `@vibe/ui` (`Button`, `Card`, `Modal`), `Section`, `Badge`,
  `Reveal`, `Field`, and `theme.css` variables. No new colours: accent for the open step and
  primary buttons, success for done, warning for waiting, muted text and border for locked.
- One primary button per screen. Money is right-aligned with tabular figures. Every control is
  a real button, link or input with a label; targets at least 44 px; text contrast 4.5:1 in
  light and dark.
- Phone first: every page works at 390 px with nothing scrolling sideways. On a phone the
  primary action and the total sit in a footer fixed to the bottom of the page.
- Words: "Your 2025 tax return", "Sign your letters", "Review your tax return", "Choose your
  package", "Pay your fee", "Sign your e-file authorization (Form 8879)". Never "delivery" on a
  client page.

### 15.2 Portal: the card (Home)

- A `Section`-style card, first on the page. Header row: title and one line of help on the
  left; on the right "N of M done", "Next: <step>", and a segmented bar with one segment per
  step present (success = done, accent = open, border = not yet).
- One row per step in an ordered list. Left: a 32 px round marker (tick on success tint when
  done; the step number on accent when open; a padlock on a bordered grey circle when locked).
  Middle: the step heading, a state badge, one muted line of detail. Right: the step's action.
- The first open step is expanded on a pale accent tint and holds its content inline: the fee
  lines and **Pay $X** for the fee, **Open eSignature** and **I've signed it** for 8879. Letters
  and package open their own pages from a **Sign** or **Choose** button.
- Done rows collapse to one line with a link: "Signed copies (2)", "View", "Signed selection",
  "Receipt".
- Footer strip: the firm's phone line and the preparer.

| Step | Open row | Done row |
|---|---|---|
| Letters | "2 letters to sign" · **Sign** | "Signed Nov 24 by Jordan and Casey" · Signed copies (2) |
| Return | file name, pages · **View**, **Download** | "Opened Nov 24" · View |
| Package | "Three packages to choose from" · **Choose** | "Advantage, with Mid-year planning meeting · signed Nov 24 by Jordan" · Signed selection |
| Fee | lines, total · **Pay $1,642.68** · "Card or bank account, on Stripe's secure page." | "$1,642.68 · Nov 24 · bank account" · Receipt. Waiting: "Payment started", amber |
| 8879 | help line · **Open eSignature** · **I've signed it** | "Confirmed Nov 25". Waiting: "Waiting for the firm to confirm" |

### 15.3 Portal: Sign page

- Top bar: back link to the card; "Step 1 of 5 · Letter 1 of 2".
- Two columns that wrap to one: the letter (PDF.js pages on a grey ground, page count,
  Download) and a side column.
- Side column, top to bottom: **Letters to sign** (list, the current one outlined, "1 of 2
  signed"); a done line for anyone who has signed; the active signer's panel with an accent
  border.
- Active signer's panel: heading "<Name>, your turn" for a second signer, with "We texted a
  code to the mobile number ending 0142" and a 6-digit field, "Code accepted", "Email it
  instead"; **Draw your signature** pad, 130 px tall, with a baseline and **Clear**; the
  agreement checkbox; **Sign as <Name>** (disabled until code, drawing and tick are all there);
  one muted line on what is recorded.
- The signature appears on the letter preview at its block as soon as it is made; the other
  signer's block reads "<Name> signs next" on a dashed line.

### 15.4 Portal: Package page

- Heading "Choose your income tax advisory package", one line of help, "Download as a PDF".
- Desktop: one column per package in a 3-column grid. Each column is a card: a 26 px tag strip
  ("Recommended" in dark, else empty), then a label holding a radio, the package name (18 px
  bold) and its price (24 px bold), then the plan's service groups in the same order in every
  column, each with its bullet lines, equal row heights so groups line up across columns. The
  chosen column has an accent border and tinted header. Nothing is chosen at first (T28).
- **Optional add-ons, with any package**: a 2-column grid of labelled checkboxes with name, one
  line, and "+ $X". All unticked at first.
- **Your selection**, accent border, two columns: the lines and "Total due in step 4" on the
  left; on the right "Sign for <package>", the signature pad, **Sign and continue to payment**,
  and "Signed as <name>". For Advantage and Concierge, a link "Read the <package> addendum
  (N pages)" opens the PDF above the pad and must be opened before the button works.
- Phone: the packages as a radio list (name, tag, price), the chosen one's groups under it, a
  fixed footer with the package, total and **Next: add-ons and signature**; add-ons and the pad
  follow on the next screen.
- Base fee and add-ons mode: the same page without the package grid; the base fee shows as a
  fixed, ticked "Included" line.

### 15.5 Vibe Files: Deliver tax return window

- egui window, about 1200 × 820, resizable. Title "Deliver tax return — <client> (<ID>)".
- Strip under the title: Tax year, Return type, Card title, and the delivery's status pill.
- Left, four bordered sections stacked, each headed by a tick, its name and a state pill:
  **Letters to sign** (template set; a tick per letter; "Who signs" with a tick per signer;
  Preview letters…), **Tax return PDF** (path field, Choose…, "Opens after" ticks), **Package
  and fee** (mode radios; pricing plan; "This client's return fee"; a table Offer / Package /
  Plan rule / Client sees, the last column editable; add-ons with ticks; Mark paid…), **Form
  8879** (link field, Paste, "Opens after" ticks, Confirm signed).
- Right pane, 360 px: **What has happened** (newest last, time right-aligned), **Tell the
  client** (notices tick, "Remind every N days", Remind now), **Kept in the client folder**
  (one line naming `Income Tax\{Year}\Delivery`, Download report…).
- Footer: Cancel delivery… on the left in the danger colour; Close and **Send to client** /
  **Save changes** on the right.
- A price that differs from the plan's worked-out value is outlined in the accent colour.

### 15.6 Vibe Files: Admin pages

- **Letter templates**: left list grouped by return type with New letter… and New return
  type…; centre toolbar (Heading, B, I, List, Table, Insert field, Insert signature, saved
  state) over the editor, fields shown as chips and signature blocks as dashed boxes; right
  pane with the field list, "This letter" settings (letterhead, default tick, Filed as, Only
  for), Preview with a client…, Copy to another return type…, Save.
- **Pricing plans**: left list of plans by tax year and return type, New plan…, Copy to next
  year…; tabs **Packages** and **Add-ons**. Packages is a grid with a column per package and a
  row per service; the first rows are the package name, "Price over the return fee" (percent
  field, "at least" amount field) and the Recommended radio; a cell edits in place, one line
  per bullet; Add package, Add service. Add-ons is a table (drag handle, Name, What the client
  reads, Price, Offered unless unticked). Right pane: "As the client sees it" for a sample
  return fee, and where the plan is used.
- **Deliveries** list: a table of open deliveries (Client, Year, Type, Step waiting, Days,
  Assigned, Last notice) with filters by step and staff; a row opens its Deliver window.

### 15.7 Canvas screens and their status

| Screen on the canvas | Status against this addendum |
|---|---|
| Client home — tax return card | Layout current. Shows four steps; the card now has five (package before fee) |
| Step 1 — sign the letters | Current |
| Card on a phone — step 4 | Layout current. Step numbers predate the package step |
| Vibe Files — Deliver tax return | Current, except the fee section, which follows "packages in the plan and in a delivery" |
| Vibe Files — Admin › Letter templates | Current; add Table and "Only for" |
| Step 3 — base fee and add-ons; Add-ons on a phone; Step 3 after paying | Current for the add-ons mode; the step is now numbered 3 (choose) and 4 (pay) |
| Engagement letter — Fee options | Current |
| Vibe Files — fee from a pricing plan; Admin › Pricing plans | Current for add-ons; packages add the grid tab |
| Step 3 — choose a package, add-ons, sign; Packages on a phone | Current. Shown with Advantage chosen for illustration; the real page starts with nothing chosen |
| Card with five steps — package before fee | Current: this is the card's final shape |
| Vibe Files — packages in the plan and in a delivery | Current |

## 16. Balance owed card

Clients sometimes ask to pay a balance that has nothing to do with a return being delivered.
Staff enter the balance in Vibe Files; the portal shows a card with one button to pay it through
Stripe; the card can be set to leave by itself after a number of days. It uses section 5's
Stripe settings and webhook and nothing else of a delivery.

### 16.1 Decisions (owner Q&A, 2026-11-24)

| # | Topic | Decision |
|---|---|---|
| B1 | Amount | The amount staff entered, paid in full. No partial payment, no client-typed amount |
| B2 | How many | One balance per client at a time, with a short memo. Entering a new one replaces an unpaid one |
| B3 | Who sees it | Every active portal user of the client |
| B4 | Show for X days | Optional per card: "Remove after N days". The firm sets the default number and whether the tick starts on. Unticked: it stays until paid or cleared |
| B5 | At expiry | The portal removes the card and the balance clears in Vibe Files. Staff enter it again to show a new card |
| B6 | After payment | The card shows "Paid" with a Receipt link for a firm-set number of days (default 7), then leaves. Vibe Files clears the balance and records the payment |
| B7 | Filing | When paid: a receipt PDF with a separate log page, filed in `Billing\{Year paid}` (created if missing) |
| B8 | Notices | Staff choose per card: "Tell the client" and "Remind every N days" |
| B9 | With a delivery | Independent. A client may have a tax return card with an unpaid fee and a balance card at once; each is paid on its own |
| B10 | Entry | The client panel in Vibe Files, and a firm-wide Balances list |
| B11 | Bank payments | "Payment started" until the bank clears it, then "Paid". It cannot be paid twice. A failure returns the card to unpaid and tells staff. The expiry clock keeps counting while a payment is pending |
| B13 | Notices | The email and text state the amount and the memo |
| B14 | During a pending payment | Staff may still change or clear the balance (16.2, Pending payments) |
| B16 | Payment notice to staff | Each balance has an assigned staff member, told by email when a payment is made, started, fails, or the card expires unpaid (16.2, Staff notices). The same holds for a delivery's fee step |
| B15 | Build order | First: the Stripe settings and webhook, then this card, before any delivery work |
| B12 | Switch | Part of Payments: shown only when the firm has Stripe set up and `firm.balances_enabled` is on (off until an admin turns it on) |

B11 differs from the delivery fee step on purpose: there, "started" opens the next step (T26);
here nothing waits on the payment, so the card simply tells the truth about it.

### 16.2 Portal

- **Card** (`pages/Balance.tsx`, on Home under the tax return card and above To do): heading
  "Balance due", the memo, the amount, **Pay $X**, "Card or bank account, on Stripe's secure
  page.", and, when it expires, "Available here until <date>".
- **States**

| State | Shown as |
|---|---|
| Due | Amount and **Pay $X** |
| Payment started | Amber: "Payment started <date>. Bank payments take a few business days. You do not need to do anything." No button |
| Paid | Green: "Paid <date>" and **Receipt**; leaves after `firm.balance_paid_days` |
| Expired, cleared, replaced | No card |

- **Table `balance`**: `id`, `client_id`, `amount_cents`, `memo` (120 characters), `state`
  (due / started / paid / expired / cleared / replaced), `expires_at` (null = no limit),
  `notify`, `remind_days`, `assigned_staff_id`, `posted_by`, `posted_at`, `paid_at`, `method`,
  `session_id`, `receipt_upload_id`. One row in `due` or `started` per client (partial unique
  index). Its actions go to an append-only `balance_event` like `delivery_event`.
- **Checkout**: `POST /api/portal/balance/checkout` makes a Stripe Checkout Session for the
  row's amount (never an amount from the browser), with the balance id in metadata. A row in
  `started` or `paid` refuses with `balance_not_due`. The shared webhook sets `paid`, `started`
  or back to `due`.
- **Expiry**: a timer (as P109's reminders) moves a `due` row past `expires_at` to `expired`.
  The days keep counting while a payment is pending (B11), but a `started` row is never removed
  while it is pending: if the payment clears it is paid; if it fails after `expires_at` the row
  expires at once and staff are told of both. An expiry is claimed before it acts, so two
  timers never both clear one row.
- **Pending payments** (B14): while a row is `started`, staff may clear it or change it.
  *Clear*: the card leaves; the payment is still followed, and if it clears the receipt is
  filed, marked "cleared by staff before the bank confirmed". *Change memo, days or notices*:
  applied at once. *Change the amount*: the pending payment stays for the amount the client
  authorised and the new amount is posted as a new balance, due when the pending one settles;
  Vibe Files warns "A payment of $X is pending. The new amount will be due as well if it
  clears." Nothing is ever charged without the client pressing Pay.
- **Staff notices** (B16): the balance's assigned staff member gets an email when the client
  pays ("Paid"), when a bank payment is started, when one fails, and when the card expires
  unpaid. It names the client, the amount, the memo, the method and the time, and links to the
  staff page. Only staff who can see the client may be assigned (`GET /request-staff`, as
  to-do lists). With nobody assigned, the notice goes to the firm's "Payment notices go to"
  person in Settings › Payments; with neither, to the admins. A notice is sent once per event
  (claimed before sending) and logged in `balance_event`; one that cannot be delivered is shown
  on the staff page and in Vibe Files' Balances list.
- **Firm API**: `GET /balances` (all, with state and days left), `GET/PUT/DELETE
  /clients/:id/balance` (PUT posts or replaces; DELETE clears; during a pending payment they
  follow Pending payments above and answer with `pending: true` so Vibe Files can warn), `POST /clients/:id/balance/remind`. `/status`
  `features` gains `balances`.
- **Receipt PDF**: built when paid: the firm, client, memo, amount, method, date; then a
  separate log page (posted, by whom and from which PC; notices sent; card first seen; checkout
  started; paid or failed; reminders). Queued as an upload tagged `balance` for Vibe Files.
- **Settings › Payments** (admins): "Payment notices go to" (the fallback staff member), the
  Balances switch, default "Remove after N days" and
  whether it starts ticked, days a paid card stays.
- Notices state the firm's name, the amount and the memo (B13), and link to the portal's
  sign-in page, never straight to Stripe. This differs from to-do and delivery notices, which
  carry no details; the firm's Settings › Payments page says so beside the switch.

### 16.3 Vibe Files

- **Client panel**: a line "Balance owed: none" / "$240.00 on the portal, 9 days left" /
  "Payment started Nov 24" with **Balance owed…**.
- **Balance owed window**: Amount; Memo; **Assigned to** (the portal's staff who can see this
  client; starts as whoever is posting, when they have a portal staff account);  "Remove from the portal after [N] days" (tick and
  number from the firm default); "Tell the client by email and text"; "Remind every [N] days";
  **Post to portal** / **Save changes**; **Clear balance**; and what has happened. With an
  unpaid delivery fee for the client it says so in one line and still allows it (B9).
- **Balances list** (beside Deliveries): Client, Amount, Memo, Assigned, Posted, Days left,
  State; filters
  Due / Payment started / Paid this month / Expired this month; a row opens the window.
- **State follows the portal**: the fetch timer reads `/balances`. Paid: the balance clears and
  the receipt is filed. Expired: the balance clears, the client's row shows "Expired unpaid
  <date>" in the list for the month, and the footer tells every PC once.
- **Filing**: the receipt arrives through the Portal inbox with a hint (origin
  `portal_balance`, the client, folder `Billing\{Year paid}`); Intake files it as `{Date paid}
  Payment receipt - {memo}.pdf`, doc type "Payment receipt" (new, seeded).
- The amount lives on the portal only. Vibe Files keeps a copy in
  `<client>/_Vibe/balance.jsonl` for when the portal is out of reach, and never syncs it to B2.
- Audit rows: `portal_balance_post`, `_change`, `_clear`, `_remind`, `_paid`, `_expired`.
  Field names only, never amounts.
- `PortalSettings.balances` (Admin › Portal, every PC); help page `portal-balance`.

### 16.4 UI

- Portal card: the same `Section` card as the others. Left: "Balance due" heading, the memo in
  muted text, the amount at 30 px bold tabular. Right: **Pay $X** (primary, 48 px) over one
  muted line. Under both, when set: "Available here until Dec 8." On a phone the button is full
  width under the amount.
- Balance owed window: a small egui window, about 520 × 420: the fields in one column, the
  history under a rule, buttons bottom right, **Clear balance** bottom left in the danger
  colour.

### 16.5 Phases

**Portal D7 — Balance owed card** (after D0, which brings Stripe)
- [x] Migration; switch; firm API; card and its states; checkout and webhook states
- [x] Expiry timer (counts through a pending payment, never removes one); pending-payment
  rules for clear and change; paid-card days; notices and reminders
- [x] Receipt PDF with log page, queued for Vibe Files
- [x] Assigned staff and the fallback; staff notices for paid, started, failed, expired
- **Gate:** the assigned staff member gets exactly one email per event, a staff member who
  cannot see the client cannot be assigned, and an unassigned balance tells the fallback; and
  in Stripe test mode: card payment → Paid → gone after the set days; bank payment →
  Payment started (a second Pay is refused) → failed → Due again (or expired, if its date passed meanwhile); a balance cleared while pending
  still files its receipt when the bank confirms; an
  unpaid card past its date is gone and `/balances` says expired; a card with no limit stays

**Vibe Files F5 — Balance owed** (with D7)
- [x] `vf-portal::balance`; client panel line and window with Assigned to; Balances list;
  settings switch
- [x] States from the portal; clear on paid and on expiry; footer notice; local copy; audit
- [x] Receipt filed in `Billing\{Year paid}`; doc type; help page
- **Gate:** e2e: post, pay, receipt with its log page filed in Billing; post with 1 day, let it
  expire, the balance is cleared in the client panel and listed as expired

Build order: first (B15), as slice 1 in section 14, on top of D0.

## 17. Open

- Missouri and E-SIGN sufficiency of the signature record; whether one spouse's signature on the
  package addendum binds the other (T19). Owner's attorney.
- Whether a prepaid representation package needs a carrier or state-law check (see the addenda
  draft's notes).
- Whether Stripe payments should also be exported for the firm's billing system.

## 18. Review with the owner (2026-11-24)

Read for gaps and contradictions before building slice 1. Decisions:

| # | Topic | Decision |
|---|---|---|
| B17 | Pending and new amount | Resolves B2 against B14: at most one `started` and one `due` row per client (two partial unique indexes). A new amount posted during a pending bank payment is Due once the pending one settles; replacing replaces only a `due` row |
| B18 | Missed webhooks | The portal also asks Stripe for a Checkout Session's state when the client returns from Stripe's page and on a timer for any session still open; the webhook and the check apply through the same de-duplicated update |
| B19 | Refunds | Ignored: refunds live in Stripe; nothing on the portal or in Vibe Files changes |
| B20 | Card fee on balances | The firm's card fee setting (T9) applies to balances too, for card payments only; bank payments never carry it |

Build: slice 1 first (D0, D7, F5), against a stand-in Stripe (a local fake for Checkout Sessions and signed webhooks); the owner runs the check with Stripe test-mode keys later. Mockups of the balance card, the Balance owed window and the Balances list are shown for the owner's OK before they are built.

Raised for slice 2, not decided yet: when the fee step opens in `amount` mode (no step 3); a letter template changed after one of two signers signed; the tax year of step PDFs for a return filed late.

## 19. Build log and questions (full auto build, owner 2026-11-24)

The owner asked for the whole addendum built without stopping: "log questions and we will
address them later". Each question below records the default taken and where to change it.

### Slice 1, portal (P115: D0 payments, D7 balance card)

Firm API contract (firm key or staff; staff scoped by `inScope`/`mustSee`; all 404
`balances_off` while Stripe is not set up or the switch is off):

| Call | Body / answer |
|---|---|
| `GET /api/firm/v1/balances` | `{ balances: [Balance], defaults: { expireDays, expireOn }, staff: [{id, name}] }` open ones and those ended in 45 days |
| `GET /api/firm/v1/clients/:id/balance` | `{ balances: [Balance & { events: [{at, kind, words, by}] }] }` last 20 |
| `PUT /api/firm/v1/clients/:id/balance` | `{ amountCents ≥ 50, memo ≤ 120, expireDays 1–365 or null, notify, remindDays 0–60, assignedStaffId or null }` → `{ balance, pending }` |
| `DELETE /api/firm/v1/clients/:id/balance` | → `{ cleared, pending }`; 404 `no_balance` |
| `POST /api/firm/v1/clients/:id/balance/remind` | → `{ told }`; 429 after 3 a day |
| `GET /api/firm/v1/balance-staff?clientId=` | `{ staff: [{id, name}] }` |
| `GET /api/firm/v1/status` | `features` gains `balances` while on |

`Balance` = `{ id, clientId, amountCents, memo, state (due|started|paid|expired|cleared|replaced),
expiresAt, daysLeft, notify, remindDays, remindedAt, assignedStaffId, postedBy, postedAt,
startedAt, paidAt, method (card|bank|null), cardFeeCents, endedAt, endedBy, clearedPending,
receiptUploadId }`. The receipt is an ordinary upload (`GET /uploads`) named
`<yyyy-mm-dd> Payment receipt - <memo>.pdf`, with the log page, tagged `balanceId`.

Client API: `GET /api/portal/balance`, `POST /balance/checkout {balanceId, method}` → `{url}`
(409 `balance_not_due`, 409 `balance_waits`), `POST /balance/return {sessionId}`,
`GET /balance/:id/receipt.pdf` (no log page). Webhook `POST /api/stripe/webhook` (raw body,
signature within 300 s, events de-duplicated in `stripe_event`). Settings › Payments:
`GET/PUT /api/staff/v1/settings/payments`, `POST …/test`, `DELETE …/keys` (admins; keys sealed,
never answered; History says only "saved").

Timers: hourly reminders; every 10 min expiry of due balances and a check with Stripe of open
checkouts (B18).

Tests: `apps/api/src/__tests__/balances.test.ts` against a stand-in Stripe server made from
Stripe's documented calls; `apps/portal/src/__tests__/Balance.test.tsx`. Not done: the owner's
check with Stripe test-mode keys (Gate stays unticked).

### Questions logged (defaults taken)

| # | Question | Default taken | Where to change it |
|---|---|---|---|
| Q1 | A bank payment fails while a newer amount was posted during it (B17). | The newer amount stands; the failed one is marked `replaced`. | `markFailed` in `payments/balance.ts` |
| Q2 | A balance cleared while its bank payment was pending, and the payment then clears. | Stays `cleared` with `paidAt` and a receipt (note on the receipt); staff are told it was paid. | `markPaid` |
| Q3 | Who is the receipt's upload filed under when the payer is unknown (paid by a webhook only)? | The first person with access to the client. | `fileReceipt` |
| Q4 | Card fee with both methods on: one Pay button or two? | Two buttons (bank account first, then card with the fee shown) so the fee is known before Stripe's page; one button when the fee is off. | `pages/Balance.tsx` |
| Q5 | Which earlier Checkout Sessions count? | Only the one last made for the balance; an older one left open is ignored. | `applySession` |
| Q6 | Reminder text says the amount and memo (B13) by email and text. | As B13; no link to pay directly, only the portal address. | `tellClient` |
| Q7 | Turning balances on with no saved key. | Refused (409 `stripe_not_set_up`); removing the keys turns balances off. | `payments/settings.ts` |
| Q8 | A Stripe restricted key (`rk_`) is accepted as well as `sk_`. | Accepted; Test checks it can list Checkout Sessions. | `payments/settings.ts` |

### Slice 1, Vibe Files (F5: balance owed; PA-107, Q-152)

Built 2026-11-24 on branch `delivery`:

- `vf-portal::balance`: the firm API calls (`balances`, `client_balances`, `post_balance`,
  `clear_balance`, `remind_balance`, `balance_staff`), `Status::has_balances`, money in and
  out, and the copy `<client>/_Vibe/balance.jsonl` (one line per change as read; days left
  are not a change). `PortalSettings.balances` (Admin › Portal, every PC, off by default).
- Client panel: "Balance owed: …" line and **Balance owed…**; the window (Amount, Memo,
  Assigned to, Remove after N days from the firm's default, Tell the client, Remind, Post /
  Save changes, Remind now, Read again, Clear balance with a confirm, what has happened; the
  copy shown "as last read" when the portal is out of reach). Admin › Portal › **Balances**
  (filters Due / Payment started / Paid this month / Expired this month / All; a client opens
  the window).
- Every PC reads `/balances` every 5 minutes; reads (and the window's) are copied with audit
  rows `portal_balance_copy`, and the first PC to see one paid or expired writes
  `portal_balance_paid` / `portal_balance_expired` (field names only). Window actions audit
  `portal_balance_post`, `_change`, `_clear`, `_remind`.
- Receipt: the portal's upload carries `balance.balanceId`; the Portal inbox writes an Intake
  hint (doc type **Payment receipt**, origin `portal_balance`, year paid); a managed Filer
  rule (`vf-capture-portal-payment-receipt`, first in the active profile) files
  `<yyyy-mm-dd> Payment receipt …` into `{Client}/Billing/{Year}`; Intake offers the year
  paid to the row (the memo may name another year: PA-89 would ask). The doc type is added
  (folder `Billing/{Year}`, client visible) when Admin › Portal saves Balance owed ticked.
- Help `portal-balance`. Live walkthrough `balance_walkthrough` (ignored test; portal on a
  real Postgres, a stand-in Stripe, the client in headless Edge via
  `scripts/balance-walkthrough/client.mjs`): post → client card → Pay → Stripe's webhook →
  Paid with Receipt → receipt in the Portal inbox with its hint → panel "none", copy and
  audit → Intake row Ready in `Billing\2026`. It found one portal bug (dates in raw SQL,
  fixed).

Gate: the e2e part is met against the stand-in (post, pay by card in the client's browser,
receipt with its log page committed by Intake into `Billing\2026` as a Payment receipt). Not
met yet: the owner's check with Stripe test-mode keys (both gates), and the expiry half run
live (covered by the portal's test of
expiry and by Vibe Files' test of the copy; a live run needs a day to pass or a database
edit).

### Questions logged, Vibe Files (defaults taken)

| # | Question | Default taken | Where to change it |
|---|---|---|---|
| Q9 | "Assigned to" starts as whoever is posting (§16.3), but a Windows login is not a portal staff account. | Starts as Nobody (the Settings › Payments person, else admins). | `BalanceWindow::new` |
| Q10 | The expiry notice: footer or notice bar? | Browse's notice bar, once per PC per session, for balances that expire while the app is open; the Balances list shows them for the month. | `PortalState::tick_balances` |
| Q11 | The Filer rule for receipts is added to every cabinet's active profile (it matches only the portal's receipt names). | Always present, first. | `vf_ingest::parsers::payment_receipt_rule` |
| Q12 | Which year is a receipt filed in? | The year paid (the date at the start of its name, and its hint). | inbox hint; `ui/intake.rs receipt_years` |
| Q13 | The unpaid delivery fee line in the window (B9). | Not shown until deliveries exist (slice 2/4). | `BalanceWindow::body` |
| Q14 | Payment receipt doc type client visible? | Yes (a receipt is the client's own record). | `ui/portal.rs` save |

### Slice 1 review (advisor, 2026-11-24) and what changed

A review of slice 1 at its gate found defects neither the tests nor the walkthrough saw. Fixed:

- **A payment was dropped** when it came on a session other than the balance's newest (a
  second Pay, or a new amount or Clear while the client was on Stripe's page). Now a superseded
  session is expired on Stripe (`POST /v1/checkout/sessions/{id}/expire`, docs read
  2026-11-24; only an `open` session can be), a payment on any of the balance's sessions is
  recorded whatever its state (staff told; a second payment is `paid_again`, refund one in
  Stripe), and a bank payment started on a changed or cleared balance tells staff.
- **Bank payments were unreachable** with both methods on and no card fee (one Pay button sent
  `card`). Now the card shows a button per method whenever both are on.
- **Save changes shortened the date**: `expireDays` now counts from now for a change too (the
  window sends the days left).
- **Staff scope**: the balance routes were mounted before the per-client guard; now after it.
- **The receipt reached the client**: it was listed in "Your uploads" and could be taken back;
  receipts are now left out of both (and out of the waiting-files quota).
- New tests for each; the walkthrough now commits in Intake and checks the file in
  `Billing\2026` and its catalog doc type.

Checked, not changed: the CSP needs nothing for Stripe (Pay navigates the whole page with
`location.assign`, which `form-action`/`connect-src` do not govern; checkout is a same-origin
call; no Stripe script). `<client>/_Vibe/balance.jsonl` is never sent to B2 (B2 push sends
catalog documents and named cabinet files only). The stand-in Stripe accepts any form field,
so the owner's run with Stripe test-mode keys is the only check of the parameter names sent.
Walkthrough tooling is in `Vibe-Files/scripts/balance-walkthrough/` (stand-in, client, reset;
send the reset script's output to a file, not a pipe).
