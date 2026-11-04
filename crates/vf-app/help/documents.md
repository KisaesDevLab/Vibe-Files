# Document details

The right-hand pane of Browse: setting doc type, tax year, status, source and tags, and reading a document's details, versions and legal holds.

Select one or more documents in the grid. The pane shows how many are selected, such as **3 documents selected**, then the fields below.

## The fields

- **DOC TYPE** — what the document is (W-2, K-1, engagement letter, and so on), from the firm's list. **None** clears it. See [Doc types](help:doc-types).
- **TAX YEAR** — the year the document belongs to, whatever folder it is in. **Permanent** is for documents with no year, such as formation papers or an EIN letter. The list runs from next year back about fifteen years.
- **STATUS** — **Received**, **In work**, **Final** or **Delivered**. It shows as a colored pill in the grid.
- **SOURCE** — **Client**, **Firm**, **Government**, **Third party**, or **Not set**.
- **TAGS** — any words you like, separated by commas, such as `amended, signed`.

Each field starts on **Keep**, showing the current value. When the selected documents differ, it shows **Mixed** with a count, such as **Mixed (3)**. Only the fields you change are written; the rest are left as they are on each document. Typing in **TAGS** when they are mixed replaces the tags of every selected document.

When you are done, click **Apply to 3 (one transaction)** (the number is how many are selected). The change is one transaction: **Undo last** takes it back. See [Undoing changes](help:undo).

> Changing the doc type or tax year does not move or rename the file. To move it, drag it to a folder; see [Moving, renaming and deleting](help:moving-renaming).

### Archived doc types

Doc types the firm has archived in Admin are not offered in **DOC TYPE**. A document that already has an archived type keeps it, and it shows as its current value.

## Details of one document

With exactly one document selected, the pane also shows its name and:

- **SHA-256** — the fingerprint of its content.
- **Versions** — how many earlier versions are kept.
- **Size**, **Modified** and **Path** (where it is in the cabinet).
- **Recognized** — for tax forms the app read, what it found: the form, year, payer and recipient (tax IDs show only their last four digits), and how sure it is.

### Earlier versions

Under **Earlier versions** each earlier content of the document is listed, newest first, with **Open**, **Restore** and **Save a copy…**. **Restore** brings an earlier version back; the content it replaces is kept as a version too, so nothing is lost. If there are none yet, the list says so.

## Legal holds

If a legal hold covers the selected documents, or the client shown, a red banner reads **Legal hold:** followed by the reason and the date. Nothing under a hold is disposed of or emptied from the trash while it holds.

To place one, click **Place legal hold on this document…** (or on this client, or on these documents), type the **Hold reason**, and click **Place hold**. **Release hold** asks for the admin password. See [Retention](help:retention).

## See also

- [Browsing the cabinet](help:browse)
- [Doc types](help:doc-types)
- [Search](help:search)
