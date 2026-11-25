# Addendum: annual service agreements and the AICPA engagement letters

Owner 2026-11-25. Approved for development the same day ("Build all slices"). Builds on
ADDENDUM-tax-return-delivery.md (PA-107 to PA-111, Vibe-Portal Addendum 02).

## 1. What the owner asked

- The firm signs an **annual service agreement** with a client on the portal without delivering a
  tax return or any other product.
- **Template engagement letters** for the firm's services, made from the AICPA Annual Tax
  Compliance Kit (2026; 17 .docx files supplied by the owner) and, for services the kit lacks,
  drafted new.

## 2. The owner's answers (Q&A 2026-11-25)

| # | Question | Answer |
|---|---|---|
| A1 | Services covered | Tax (the AICPA kit), **and** bookkeeping/accounting, payroll, advisory/CFO (drafted new, marked Sample, for the attorney's review) |
| A2 | Billing | Chosen **per agreement**: sign only, one-time fee or retainer at signing, or a **monthly charge** |
| A3 | AICPA/CNA text | **Built into the app** as the samples for every install (see §9, licence) |
| A4 | Optional (blue italic) provisions | **Left in the template**; the firm edits the template |
| A5 | Terms and Conditions Addendum | **Appended to each letter**, covered by that letter's signature |
| A6 | Sending | **Per client and in bulk** |
| A7 | Firm's signature | **Partner's name printed** with `/s/`; no firm signing step |
| A8 | Portal | **Same card**, agreement steps only (letters, package, pay); no return, no 8879 |
| A9 | Monthly charge runs | **Until cancelled** (by staff, or by closing the engagement) |
| A10 | Several services | **One agreement, fee lines**: a letter per service, signed together; each line one-time or monthly; one payment setup |
| A11 | Agreement already signed when the return is delivered | Deliver tax return… **skips the letters and says so**; staff can still tick them |
| A12 | Closure letter | **Sent without signature**: read-only letter on the portal, filed; it cancels the open agreement and the monthly charge |
| A13 | Renewal with another monthly amount | **The same subscription changes** to the new amount from the next charge; the client sees it before signing |
| A14 | Client address | **Left out**, as today |
| A15 | Firm's signer | **Admin list of partners** (name, title); each client defaults to its CSV Partner; changeable per agreement |
| A16 | The kit's notes to the firm inside the letters | **Deleted** at conversion |
| A17 | A fill-in left in a letter | **Send refused**, naming the letter and the fill-in |
| A18 | Licence | **Built into the app, exposure accepted**: the text ships in the public binary until v2 switches to an import of each firm's own kit |

## 3. Defaults chosen for the rest (say if any is wrong)

1. **One agreement per client per year.** The year is the service year ("2027 service agreement").
   The card title defaults to "Your 2027 service agreement".
2. **Services are the return types plus new service types.** The built-in list gains Bookkeeping,
   Payroll, Advisory/CFO, Tax projections, Tax consulting, Audit representation, FBAR (FinCEN 114),
   709, 706, 5500, 990-PF and Amended return. A letter template is offered for the services it
   names, as today.
3. **The AICPA instructions are not letter text.** The cover pages ("Overview", "How to use this
   document"), the footnotes, the CNA disclaimer and the kit's notes to the firm inside the letters
   (such as "(On the rare occasion that the firm will initiate tax payments… include the following
   provision…)") are left out (A16). The provisions those notes introduce stay as letter text (A4).
4. **Bracketed fill-ins.** Those with a field become the field: `[CPA Firm]` → `{Firm name}`,
   `[Client & Spouse Names]` → `{Client name}`, `[Date]` → `{Date}`, `20[XX]` → `{Tax year}`,
   `$[X]` (fee) → `{Fee}`, `[Accountant Name]` and `[Accountant Title]` → the new `{Partner name}`
   and `{Partner title}`. Others stay as written, for example `[X]%` or `[State]`, and are
   highlighted. **Send is refused while a letter still holds a `[…]` fill-in**, and the message
   names it.
5. **The Terms and Conditions Addendum is one template** (kind *Terms and conditions*), edited in
   one place. Each letter has **Append the Terms and Conditions** (on by default). The printed
   letter carries the addendum's text after the letter and before the signature blocks, so the one
   signature covers both. The "INITIAL HERE" line is dropped.
6. **New fields:** `{Partner name}`, `{Partner title}`, `{Services}` (the services in this
   agreement), `{Fee lines}` (a table: service, one-time, monthly), `{Monthly fee}`,
   `{One-time fee}`.
7. **Monthly charge, through the firm's Stripe** (Stripe Billing subscription; card or bank account,
   as the firm offers):
   - The client sets it up on the card's Pay step through Stripe Checkout in subscription mode.
     The one-time lines are charged at the same time as the first month.
   - Every month: a **Payment receipt** PDF is filed in `Billing/{Year}`, like a balance receipt.
     Monthly receipts have no log page.
   - A failed monthly charge: the assigned staff member is told, every PC says so once, and the
     client's card shows **Update payment method** (Checkout in setup mode).
   - Staff: **Change monthly amount** (from the next charge; the client is told) and **Stop
     monthly charge** in the agreement window.
   - Webhooks gain `invoice.paid`, `invoice.payment_failed` and `customer.subscription.deleted`;
     the setup guide's list of events is updated.
8. **Packages stay possible.** A pricing plan can be made for a service such as Bookkeeping, so the
   client can choose a level. Most agreements will use fee lines only.
9. **Bulk send** works like Invite from client list: groups of 20 with a pause, and **Stop**.
   **Fill from last year** copies each client's services and fees from their last agreement.
10. **An agreement window, not the Deliver window.** "Service agreement…" in the client's details
    opens a smaller window: year, services, letters, fee lines, partner, notify. Deliveries and
    agreements share the portal engine and the Deliveries page (a **Kind** column: Return,
    Agreement).
11. **Closure.** "End engagement…" in the client's details asks first. It names the open agreement
    and the monthly charge it will stop, then sends the closure letter (read-only card), cancels
    both, and files the letter PDF in `Correspondence/{Year}`.

## 4. UI examples (synthetic names)

### 4.1 Vibe Files: Service agreement window (client details › Service agreement…)

```
┌─ 2027 service agreement — Sample Farms LLC (1042) ─────────────────────────── ✕ ┐
│ Year [2027 ▾]   Card title [Your 2027 service agreement              ]           │
│ Signed by: Pat Sample, Managing member  (Portal contacts)     Partner [K. Kisaes ▾]│
│                                                                                    │
│ SERVICES AND FEES                                       One-time     Monthly       │
│  ☑ S corporation return (1120-S)  letter: 1120-S eng.   [ 1,850.00]  [       ]     │
│  ☑ Bookkeeping                     letter: Bookkeeping   [        ]  [  450.00]     │
│  ☑ Payroll                         letter: Payroll       [        ]  [  125.00]     │
│  ☐ Advisory / CFO                                                                  │
│  [ + Add service ]                         Total       $1,850.00    $575.00/month │
│                                                                                    │
│ Payment  (•) Pay on the portal   ( ) Sign only                                     │
│ Letters  ☑ 1120-S engagement  ☑ Bookkeeping  ☑ Payroll  ☑ Consent to use (§7216)  │
│          Terms and Conditions appended to each   [ Preview letters… ]              │
│ ☑ Tell the client by email and text   Remind every [3] days                        │
│                                         [ Fill from last year ]  [ Send to client ]│
└────────────────────────────────────────────────────────────────────────────────────┘
```

### 4.2 Portal: the client's card

```
┌ Your 2027 service agreement ──────────────────────── Kisaes CPA ┐
│ ✔ 1  Sign your letters        Signed Jan 6 by Pat Sample          │
│ ○ 2  Set up payment                                               │
│      S corporation return (1120-S)     $1,850.00 today           │
│      Bookkeeping                         $450.00 a month          │
│      Payroll                             $125.00 a month          │
│      Today $2,425.00 · then $575.00 a month until either of us    │
│      ends the agreement                                           │
│      [ Pay by bank account ]  [ Pay by card (+3%) ]               │
└───────────────────────────────────────────────────────────────────┘
After setup:  Monthly payment active · next charge Feb 6 · [ Update payment method ]
```

### 4.3 Vibe Files: Admin › Portal › Send agreements

```
┌─ Send agreements ─────────────────────────────────────────────────────────────────┐
│ Year [2027 ▾]  Show [Not sent this year ▾]  Partner [All ▾]  Search [          ]  │
│  ☑ Client                     Services (from last year)      One-time  Monthly    │
│  ☑ Sample Farms LLC (1042)    1120-S, Bookkeeping, Payroll   1,850.00   575.00    │
│  ☑ Alex & Jordan Sample (88)  1040                             640.00       —     │
│  ☐ Example Trust (310)        1041  (no portal login)              —        —     │
│ 2 ticked · groups of [20] · pause [60] s     [ Fill from last year ] [ Send 2 ]    │
└────────────────────────────────────────────────────────────────────────────────────┘
```

### 4.4 Letter editor: a fill-in

```
│ In addition, we will initiate [Frequency] estimated tax payments on your behalf…  │
│                               ^^^^^^^^^^^ highlighted: fill in before sending      │
```

## 5. Templates to be built

From the AICPA kit (17): 1040, 1120, 1120-S, 1065, 1041, 990, 990-PF, 709, 706/8971, 5500,
FinCEN 114, amended return, tax projections, tax consulting, audit representation, the Terms and
Conditions Addendum, and the engagement closure letter.

Drafted new in the kit's structure (scope, firm and client responsibilities, fees, term and
termination, signatures), marked **Sample — review before use**: Bookkeeping/accounting
(monthly, non-attest; SSARS preparation of financial statements excluded unless named), Payroll,
Advisory/CFO.

The existing samples (two engagement letters, the two §7216 consents and the two package addenda)
stay.

## 6. Work, in slices

| Slice | Repo | What |
|---|---|---|
| S1 | Vibe Files `vf-letters` | A dev converter (`tools/aicpa-import`) from .docx to templates; the 17 kit templates and 3 new ones as built-in samples; fill-in check; Terms and Conditions appended; new fields; partners (Admin › Portal › Partners, CSV Partner default); service types |
| S2 | Vibe-Portal | Agreement kind on the delivery engine; fee lines (one-time and monthly); Stripe subscriptions (Checkout in subscription and setup modes, the three webhooks, monthly receipts, failed-charge notice, change amount, stop); closure letter card; firm API for these |
| S3 | Vibe Files `vf-app` | Service agreement window; Send agreements (bulk); Deliveries page Kind column; the Deliver window's link to a signed agreement; End engagement…; monthly charge status in client details |
| S4 | both | Help articles (≤1000 words each), setup guide (subscriptions), CHANGELOG, tests, a walkthrough against the stand-in Stripe, screenshots |

Release as before, after the owner says so: portal first, then Vibe Files.

## 7. Verification

- Converter: each kit letter converts with no lost paragraph, the cover, footnotes and disclaimer
  removed, every bracket either a field or a highlighted fill-in. Test against counts of
  paragraphs and fill-ins per file.
- Portal: an agreement with one-time and monthly lines; the subscription made, renewed at a new
  amount, failed and updated, stopped; closure cancels it; webhooks are idempotent; a stale or
  wrong-amount event is refused. Use the stand-in Stripe, then Stripe test mode by the owner.
- Vibe Files: bulk send of 300 synthetic clients in groups; the Deliver window skips letters
  covered by a signed agreement; End engagement stops the charge.

## 8. Not in this addendum

- Proration, mid-month starts and refunds (Stripe dashboard).
- A client cancelling the monthly charge on the portal (they ask the firm; the closure letter ends it).
- Mailing letters (no address, A14).

## 9. Licence note (owner decisions A3, A18)

The kit's letters are © 2026 CNA, all rights reserved, made available to the firm through the
AICPA Tax Section. Built into the app, their text ships in every public binary on
`vibe-files-releases` and in the repo. The owner accepted that exposure (2026-11-25). Before
selling to other firms (v2), switch to an import of each firm's own copy of the kit.

## 10. Built (2026-11-25)

Local commits only. Vibe Files (branch `agreements`): `40a5867` addendum, `0985d6f` S1, `358d78a` S3, `fda3a41` walkthrough and window fixes, then docs. Portal (worktree `C:\Projects\worktrees\portal-agreements`, branch `agreements`): `363eda6` S2, `1281004` step PDFs tagged, `a703f4e` the portal's records left out of the client's uploads (also a fix for tax return deliveries' step PDFs, found in the walkthrough).

Defaults taken while building (say if any is wrong):

- **Year**: the agreement's year is the service year; `{Tax year}` in its letters is the year before (the returns it covers); a delivery for that return year and type finds the agreement (A11).
- **Signers**: by the client's entity type in the cabinet (none set: an individual).
- **Letters ticked**: those for the ticked services; the §7216 consents offered unticked.
- **`{Fee}` in a letter**: that letter's services' fees ("$450.00 a month").
- **The fee waits for the letters** on the card (Locked until they are signed).
- **Card fee on monthly charges**: on the first charge's one-time part and on each month, by card only; the payment method is changed only to one of the same kind.
- **A renewal with no monthly lines** leaves a running charge alone (staff stop it).
- **Cancelling an agreement** does not stop a running charge (End engagement… or Stop does).
- **The closure letter** is shared with every person of the client on the portal (as a file to read), put in the Portal inbox for Intake, and its fill-ins are typed per client in the window.
- **Bulk send** builds each client's agreement from their last one (services, fees, assigned, reminders), the letters as the templates now say, the client's partner; clients with none are sent from their own window.
- **Monthly receipts** have no log page; the first invoice files no monthly receipt (the payment record covers it).

Tests: portal api 413 (+5 skipped), portal app 159; Vibe Files vf-letters 13, vf-portal 62, vf-app agreement tests, `agreement_walkthrough` passes (screens in the walk folder).

## 11. Walkthrough of deliveries and agreements (owner 2026-11-25)

Six live walkthroughs against a fresh portal and the stand-in Stripe pass: balance, delivery,
letters, fee, agreement (a business: card, a failed month, Intake filing, End engagement) and
agreement_joint (a couple: two signers, bank payment started then cleared, the Deliver window's
link, a changed monthly amount, bulk renewal, signing in again, the one-time fee only, Stop,
Sign only and Cancel). Found and fixed: the AICPA 1040 letter ticked in Deliver tax return…
beside the firm's own (Send refused for its fill-ins); a changed monthly amount lost at renewal;
no plain words for the portal's delivery and agreement errors; no check for a spouse without
their own contact; the bulk list's small tick boxes; narrow partner boxes; agreements missing
from the portal's staff page; an agreement's fee record and receipt worded as a tax return's;
the portal's own step PDFs listed (and removable) in the client's uploads.

## 12. Phone review and the Deliveries page (owner 2026-11-25)

**Phone review.** The six walkthroughs run again at phone size (`VF_MOBILE=1`, 390×844, touch; `scripts/walkthrough/mobile.mjs` records anything wider than the screen and any tap target under 32 px). No page scrolls sideways. Fixed:
- The Sign page gains **Go to signing ↓** on a phone, since the letter, Terms and all, comes before the pad.
- An action link (`.p-link`) is 44 px tall on a narrow screen (`html[data-narrow='1']`).
- After signing an agreement the link says "Back to your agreement".
- Vibe Files writes Intake's hint before a portal file lands in the Portal inbox. Both writers now do this (`inbox::note_for_intake`); Fetch again wrote none before.

**Deliveries page (Q&A 2026-11-25).** Admin › Portal › Deliveries becomes "Deliveries and agreements":
- **Show** filters by the step waiting (Tax return, Form 8879, Letters to sign, Package to choose, Payment due), by kind (Tax returns, Agreements), and **Monthly charge failed**. The failed filter lists the agreement that set the charge, open or finished, whatever the season.
- **Show finished** adds those finished in the season picked: a tax return by its tax year, an agreement by the tax year of the returns it covers (its `return_year`, else its year − 1).
- **Waiting too long after N days** is set by the firm, default 14. It is kept in `_Vibe/deliveries.json` for every PC, not in the portal settings, which Admin › Portal rebuilds on each save. Its audit row is `deliveries_overdue_days`.
- A **Monthly charge** column shows the amount, or "Failed <day>" in red. ⚠ is not used, because the fonts may lack it.
- A count line: listed, open, and how many are waiting too long.
- The rules are in `vf_portal::worklist`, with unit tests. The live agreement walkthrough's Step 2b checks the failed month on this page.
