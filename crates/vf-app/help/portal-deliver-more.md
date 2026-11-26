# Delivering statements and other documents

**Deliver…** in a client's details sends more than a tax return. Its **Kind** picker chooses what the card on the client's portal delivers:

- **Tax return**: as before. Letters, the return, the package and fee, and Form 8879.
- **Financial statements**: letters, the statements and the fee.
- **Other document**: letters, the documents and the fee. Type **What it is** (for example *Payroll reports*, up to 24 letters, digits or spaces). The card is named after it.

Form 8879 and the package step are only for a tax return. The fee for statements or another document is one amount.

This needs Vibe Portal 0.5.6 or later. On an older portal, **Send to client** says to update the portal first and nothing is left on the client's portal.

## Several PDFs on one card

Under the documents step, pick the first PDF, then **+ Add another PDF** for each more (a cover letter, the notes), up to ten. **Remove** takes one off before it is sent.

The client sees each PDF on the card and opens or downloads it there. The step is done once **every** PDF has been opened; the window says how many have been, and reminders keep going until then. Once the client has opened them, the PDFs cannot be changed: send changed or added ones on a new card. (A single tax return can still be revised as before.)

## Pay first

With the fee step ticked, tick **Documents open only after the fee is paid**. The client then sees the documents listed but locked, marked to open when they pay, beside the **Pay** button. They open:

- when the client pays by card;
- when the client starts a bank payment (as Form 8879 does: a bank payment that fails later still leaves the fee owed);
- when you use **Mark paid…** in the window.

A payment of the wrong amount does not open them: check it in Stripe first. The tick can be changed until the fee is paid or the documents are opened.

## Where the step PDFs are filed

Each finished step's PDF, with its log page, comes back to the Portal inbox and Intake files it:

- a tax return's in `Income Tax\<year>\Delivery`;
- financial statements' in `Financials\<year>\Delivery`;
- another document's in `Other\<year>\Delivery`.

Each kind has its own doc types (*Statements delivery — documents record*, *Document delivery — fee receipt*, and so on), added when a cabinet with deliveries turned on is opened.

## On the Deliveries page

**Admin › Portal › Deliveries** lists every kind. The **Kind** column says *Financial statements* or the document's name; **Tax returns** shows tax returns only. Clicking a row opens the card in **Deliver…**.

Update every PC to this version: an older Vibe Files lists only tax return cards and does not file a statements card's PDFs in Financials.
