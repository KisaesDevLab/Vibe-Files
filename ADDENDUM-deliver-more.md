# Addendum: delivering more than a tax return, pay-first documents, files held for a balance

Owner 2026-11-26 (PA-114; Vibe-Portal Addendum 04, QUESTIONS P119). Builds on
ADDENDUM-tax-return-delivery.md (PA-107 to PA-111) and ADDENDUM-annual-agreements.md (PA-112).

## 1. What the owner asked

- "Expand to allow the option to deliver other items, such as a financial statement."
- "Add a mark to require payment before delivery."
- On Balance owed: "Add option to lock file view/download and share until balance is paid."

## 2. The owner's answers (Q&A 2026-11-26)

| # | Question | Answer |
|---|---|---|
| M1 | How other items are sent | **One window with a Kind picker**: the client's button becomes **Deliver…**; Kind is Tax return, Financial statements or Other document; steps are letters, the documents (one or more PDFs) and the fee; Form 8879 (and the package step) only for a tax return |
| M2 | What "pay first" locks | **The documents, per card**: a tick on the fee, "Documents open only after the fee is paid". Form 8879 keeps its own lock |
| M3 | What a balance holds | **Everything shared** with the client: while the balance is due they see their files listed but cannot open, download or share any; files shared meanwhile wait too. Paying (or Clear balance) releases them. Their uploads to the firm still work |
| M4 | What the client sees of held files | **Listed, locked, with a Pay button**: "Opens when your balance is paid" |
| M5 | A holding balance that is removed after N days | **Removal releases the files** (paid or not) |
| M6 | A bank payment started but not cleared | **Opens the documents and releases the files**, as Form 8879 opens today |
| M7 | Several PDFs on one card | **Done when every PDF is opened** (viewed or downloaded); each shows Opened |
| M8 | The client's view (mockup) | Approved: the card's steps say "financial statements", not "return"; locked items say why and offer Pay |

## 3. The contract (portal 0.5.6)

Firm API (`/api/firm/v1`):

- `POST /deliveries`: `kind` is `return`, `agreement`, `statements` or `document`. A statements or
  document card takes the letters, documents (`steps.return`) and fee steps only: package or
  Form 8879 is `400 kind_steps`. A statements card's `returnType` is stored as `Financial
  statements`; a document card's `returnType` is the item's name the firm types (as today's
  pattern, 24 characters). Default titles: "Your {year} financial statements", "Your {year}
  {name lower-cased}".
- `locks.returnAfterFee` (default false) on create and PATCH; echoed in the delivery JSON. A
  portal that does not echo it is older: Vibe Files cancels the card and says to update the
  portal.
- `PUT /deliveries/:id/files { fileIds: [uuid, …] }` (1 to 10, the client's files on the portal):
  the card's documents in order. `409 files_seen` once any has been opened (a tax return is
  revised with `PUT /return` as today). `PUT /return` keeps working and sets the one file.
- The delivery JSON gains `files: [{ id, name, sha256, openedAt }]`; `returnFile` stays the first.
- `GET /deliveries?kind=` also takes `statements` and `document`; the default is still `return`.
- A balance (`PUT /clients/:id/balance`) takes `holdFiles` (default false) and echoes it.
- Step PDFs (Vibe Files' Intake files them by name): a statements card's are named
  `<year> Statements <n> - <step>.pdf`, another document's `<year> Document <n> - <step>.pdf`
  (a tax return's stay `<year> Delivery <n> - …`, an agreement's `<year> Agreement <n> - …`), and
  the upload's `delivery.kind` says `statements` or `document`.

Client side:

- The card's documents step is headed by kind ("Review your tax return", "Review your financial
  statements", "Review your documents"); `files: [{ id, name, size, opened }]` once open,
  `lockedFiles: [name]` while locked; `after` lists step 4 when the fee lock holds it.
- A document is locked while the letters lock holds (as today) or while `returnAfterFee` holds:
  the fee step present, not done and no bank payment started.
- `GET /api/portal/files` marks each file `held` and gives `held: { amountCents }` while a due
  balance with `holdFiles` exists; opening, downloading, previewing and making or using an
  outside link of a held file are refused (`423 balance_held`). A bank payment started (balance
  `started`), paid, cleared, expired or replaced releases them.

## 4. Vibe Files

- Client details: **Deliver…** (was Deliver tax return…). The window's Kind picker; **Documents**
  takes one or more of the client's PDFs (+ Add another PDF); the fee's tick "Documents open
  only after the fee is paid"; Form 8879 and Package only for a tax return.
- Filing by kind: a tax return's step PDFs in `Income Tax\{Year}\Delivery` (as today); financial
  statements' in `Financials\{Year}\Delivery`; another document's in `Other\{Year}\Delivery`
  (their own doc types, added when deliveries are turned on).
- Balance owed: "Hold the client's files until this is paid", with the words of M3.
- Deliveries page: the Kind column says Financial statements or the document's name; Tax returns
  filter is tax returns only.
- Every PC must be updated: an older Vibe Files lists only tax return cards and would not file a
  statements card's PDFs in Financials.

## 5. After the walkthrough (owner 2026-11-26)

- **A finished card** opens in Deliver… read-only (Complete, its documents, fee and log; "Finished
  <time>. Nothing here can be changed.") with **Send another** for a new card of the same kind
  and year. The client panel keeps the newest finished card's line ("…: complete") when none is
  open. Before, it opened as an empty "Not sent yet" form that could be sent again.
- **A balance hold spares paid deliveries**: the documents of a card whose fee is paid stay open,
  downloadable and shareable while the balance holds the client's other files (Vibe Portal 0.5.8).
- **Signed on paper** (owner 2026-11-26): **Signed on paper…** in the Letters to sign and Form 8879
  sections marks the step done with a note (portal `POST /deliveries/:id/paper-signed`, 0.5.8);
  the log and step PDF say so. Help `portal-delivery-changes`.
- **A corrected return after payment** (owner 2026-11-26): Save changes no longer resends a fee
  the client paid (the portal refused it, `409 fee_paid`, so a revision after payment failed);
  a finished card has **Revise the return…** (it opens again, the fee stays paid); **Send
  another** after a paid card starts with Fee unticked and says why. `revise_walkthrough` drives
  the owner's sequence: send, view, pay, revise, Updated with no new bill, 8879 on paper, then the
  finished card revised.
- **Form 8879 at the office or by mail; bulk signed on paper** (owner 2026-11-26): Deliver…'s
  Form 8879 has **How it is signed** (eSignature link, pick up and sign at our office, we mailed
  the return; portal `esignMode`, 0.5.8). Admin › Portal › Deliveries ticks rows waiting on
  their letters or Form 8879 too, and **Mark ticked signed on paper…** marks each one's waiting
  step with one note (the others are skipped and named). `revise_walkthrough` steps 8-10.
