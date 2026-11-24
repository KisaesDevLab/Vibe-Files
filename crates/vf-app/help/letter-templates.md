# Letter templates

The letters a tax return delivery sends to be signed on the portal: engagement letters, consents to disclose or use tax return information, and package addenda. They are kept in the cabinet (`_Vibe\letters`) for every PC, and each save is a new version; a delivery remembers the version it sent.

Open **Admin › Portal › Letter templates**.

## The samples

The first time, Vibe Files adds samples: an engagement letter for a 1040, one for an entity (1120, 1120-S, 1065), a consent to disclose, a consent to use, and the Advantage and Concierge package addenda. Their text is a starting point only: each says **Sample — review before use** on the page until you save your own text. Paste your firm's engagement letter and your §7216 consent wording; confirm the consents' wording against Treas. Reg. §301.7216-3 and Rev. Proc. 2013-14, and have your attorney review the addenda.

## Editing a letter

Choose a letter on the left, or press **New letter…**.

- **Name the client sees**, the **Return types** it is offered for (none ticked: every return), **Kind** (a letter, or a package addendum and its package), **Filed as** (the doc type), **On the firm's letterhead** and **Ticked in a new delivery**.
- The letter is a list of blocks: **Heading**, **Paragraph**, **List** (one item a line), **Table** (a row a line, cells split by `|`, the first row bold) and **Signature**.
- In the text, `**bold**` and `*italic*`; **Insert field** puts a field such as `{Client name}`, `{Tax year}` or `{Fee}` into the block you last edited. A field with no value shows as written, so Preview shows what is missing.
- **only for**: a block can be kept only for some return types or packages (`1040`, `1065`, `Advantage`), so one letter serves several returns.
- A **Signature** block is a line with the signer's name under it and a date line: Taxpayer, Spouse (left out when the return has none), or Officer or partner with their title.

**Preview for a 1040…** (and the other return types the letter is offered for) lays it out for invented people and opens it. **Save** keeps it as the next version. **Delete…** removes it; deliveries already sent keep their copy.

## In a delivery

In **Deliver tax return…**, tick **Letters to sign**: the letters for the return type are listed, ticked as their template says. **Who signs** comes from the client's Portal contacts: the taxpayer and spouse on a 1040, one person with a title for an entity. A second signer needs their own email or mobile: their code goes there. **Preview letters…** shows the letters as the client will see them.

On the portal the client reads each letter, draws a signature, ticks the agreement and signs; the spouse signs next on the same device after typing a code sent to them. When everyone has signed, the signed copies go to the client's files, and the step's PDF (the signed letters, then the log of who signed when, from where) is filed in the client's `Income Tax\<year>\Delivery` folder.

Once anyone has signed, the letters and who signs are fixed: to change them, cancel the delivery and send a new one.
