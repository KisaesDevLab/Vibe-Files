# Letter templates

The letters a tax return delivery sends to be signed on the portal: engagement letters, consents to disclose or use tax return information, and package addenda. They are kept in the cabinet (`_Vibe\letters`) for every PC, and each save is a new version; a delivery remembers the version it sent.

Open **Admin › Portal › Letter templates**.

## The samples

The first time, Vibe Files adds samples: an engagement letter for a 1040, one for an entity (1120, 1120-S, 1065), a consent to disclose, a consent to use, and the Advantage and Concierge package addenda; and, for [annual agreements](help:portal-agreements), the AICPA Annual Tax Compliance Kit's engagement letters, its Terms and Conditions and closure letter, and letters for bookkeeping, payroll and advisory. Their text is a starting point only: each says **Sample — review before use** on the page until you save your own text. Confirm the consents' wording against Treas. Reg. §301.7216-3 and Rev. Proc. 2013-14, and have your attorney review the letters.

A `[…]` fill-in left from the kit, such as `[X]%` or `[State]`, is highlighted in yellow and listed under **This letter**; a letter that still has one is never sent. **Terms and Conditions** (a kind of its own) is put where a letter says `{Terms and Conditions}`; a **Closure letter** is sent by End engagement…, read and not signed.

## Editing a letter

The letters are listed on the left by the return types they are for. Choose one, or press **New letter…**. **New return type…** under the list adds one of your own (a state return such as CA 540), signed by the taxpayer and spouse or by an officer or partner; letters, pricing plans and **Deliver…** then offer it. Tick it on a letter to offer the letter for it. The middle shows the letter on a page as it prints; click in it and type.

- **Enter** starts a new paragraph (in a list, a new item); **Shift+Enter** breaks the line. Backspace in an empty list item removes it.
- The toolbar: **Heading** turns the paragraph the cursor is in into a heading (or back); **B** and **I** make the selected words bold or italic (with nothing selected, what you type next); **List** turns the paragraph into a list, a line an item (or back); **Table** adds a table of cells, with **Add row**, **Add column**, **Remove row** and **Remove column** under it, the first row bold; **Insert field** puts a field such as `{Client name}`, `{Tax year}` or `{Fee}` where the cursor is; **Insert signature** adds a signature block (Taxpayer, Spouse, or Officer or partner). Right of the toolbar: **Saved** or **Not saved**.
- Bold and italic text keeps faint `**` and `*` marks around it on the page; they are not printed. Fields show as blue chips; a field with no value prints as written, so Preview shows what is missing.
- Signature blocks show as dashed boxes. On the printed letter each is a line with the signer's name under it and a date line; a spouse's is left out when the return has none.

On the right:

- **This letter**: **Name the client sees**, the **Return types** it is offered for (none ticked: every return), **Letter** or **Package addendum** and its package, **Filed as** (the doc type), **On the firm's letterhead** and **Ticked in a new delivery**.
- **This block** (the one the cursor is in, marked on the page's left edge): **Only for** some return types or packages (`Advantage`), so one letter serves several returns; **Move up**, **Move down**, **Remove**; for a signature block, who signs.
- **Fields**: click one to put it where the cursor is.
- **Preview…** lays the letter out as the return type chosen beside it and opens it, **with** invented people, or with a client you find in the box below (their name, ID and signers from their Portal contacts; the fee fields show as written outside a delivery). **Copy to this return type** makes a new letter with this text for the return type chosen (Save keeps it). **Save** keeps the letter as the next version; empty list items and table rows are left out. **Delete…** removes it; deliveries already sent keep their copy.

Previews are written in the `cache\letters` folder of Vibe Files' data and emptied when Vibe Files closes.

## In a delivery

In **Deliver…**, tick **Letters to sign**: the letters for the return type are listed, ticked as their template says. **Who signs** comes from the client's Portal contacts: the taxpayer and spouse on a 1040, one person with a title for an entity. A second signer needs their own email or mobile: their code goes there. **Preview letters…** shows the letters as the client will see them.

On the portal the client reads each letter, draws a signature, ticks the agreement and signs; the spouse signs next on the same device after typing a code sent to them. When everyone has signed, the signed copies go to the client's files, and the step's PDF (the signed letters, then the log of who signed when, from where) is filed in the client's `Income Tax\<year>\Delivery` folder.

Once anyone has signed, the letters and who signs are fixed: to change them, cancel the delivery and send a new one.
