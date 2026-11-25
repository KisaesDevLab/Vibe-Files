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
