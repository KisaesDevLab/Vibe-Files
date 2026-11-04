# Barcode cover sheets and batch scanning

How to print cover and separator sheets, scan many documents in one stack, and how Vibe Files splits the stack and files each document. Also how to make sheets in another program.

## Printing a cover sheet

A cover sheet goes on top of a document and tells Intake whose it is. Open it from **Cover sheet for client…** at the top of the **Intake** tab, or **Print cover sheet** in Browse (which starts with the client of the folder shown).

- **Client**: only clients with an ID in the client list are offered, for example `Baker Farms (1004)`.
- **Doc type**: starts with the doc type ticked **Default on cover sheets** in Admin › **Doc types** (see [doc types](help:doc-types)), or **Not set**.
- **Tax year**: starts at last year. Type another year, `Permanent`, or leave it empty.
- **Sheets**: how many copies, 1 to 50. Use one sheet per document.

The folder the document will go to is shown below. Press **Create cover sheet**: the PDF opens in your PDF program; print it from there.

**Print 10 blank separators** prints sheets that only mark where a new document starts, with no client.

## Scanning a stack

1. Put a cover sheet (or a separator) on top of each document.
2. Stack them all and scan into the Scans folder (see [Scanning](help:scanning)).
3. The stack is split and each document appears in Intake's queue. Check the rows and commit (see [Filing documents with Intake](help:intake)).

How the stack is split:

- Each page is checked for a sheet's barcode. A sheet starts a new document and is not filed itself.
- A blank page right after a sheet (its back, in a both-sides scan) is dropped too. Blank pages inside a document stay.
- Pages before the first sheet become a document of their own.
- After a cover sheet, the document is named for the client, for example `Baker Farms_1004_2025 W-2.pdf`, and its doc type and tax year are set when it is filed.
- After a separator, or a cover sheet for an ID not in the client list, the piece is named `<scan name> part 2.pdf` and Intake finds the client the usual way.

## Making sheets in another program

Every sheet carries one line of text, printed twice: as a QR code and as a Code 128 barcode. Either one is enough. To make your own, print one of these as a QR code or a Code 128 barcode:

- Cover sheet: `VF1|C|<client ID>|<doc type id or empty>|<year, P, or empty>`
- Separator: `VF1|S`

For the fields:

- Client ID: the ID from the client list, letters and digits only, never a `|`. Example: `VF1|C|SMIT1001||2025`.
- Doc type id: the `id` of the doc type in the cabinet's `_Vibe\doc_types.json`, with or without hyphens. Leave it empty for none. An id that is not in the list is ignored.
- Year: a year such as `2025`, `P` for Permanent, or empty.
- Always write all five parts, keeping the `|` marks even when a field is empty.

> Text that does not start with `VF1|` is not a sheet: the page is kept as part of the document.
