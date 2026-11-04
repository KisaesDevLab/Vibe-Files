# The Just filed panel

After Intake files a batch, the Just filed panel lists the new documents so you can set each one's doc type, tax year, status and source in one place.

## When it appears

Each time Intake files a batch (from **Preview**, or a zip from **Import**), a panel titled Just filed opens at the top of the Intake tab, for example `Just filed: 12 documents`. It lists the documents of the last batch, one row each.

Documents that came from a [barcode cover sheet](help:cover-sheets) or from [print capture](help:print-capture) already have their doc type and year. The others usually need them: Intake files by folder and name, and only guesses the tax year from the folder (an `Income Tax\2025` folder gives 2025).

## The columns

- **DOCUMENT**: the file name as filed.
- **DOC TYPE**: pick one from the firm's list (see [doc types](help:doc-types)). **Needs doc type** means none is set.
- **TAX YEAR**: type a year such as `2025`, or `Permanent` for a document with no tax year. An empty box also means Permanent. A year in red is not valid: **Apply** stops at that row and says why.
- **STATUS**: **Received**, **In work**, **Final** or **Delivered**.
- **SOURCE**: **Client**, **Firm**, **Government**, **Third party**, or `—` for none.

## Setting the whole batch

The **Set all** row above the list has the same four boxes, filled in from the first row. Change the ones you want, then press **Set all rows**. Only the boxes you changed are copied to every row; the others are left as each row has them. For example, change only the tax year to `2025` and press **Set all rows**: every row gets 2025, and each keeps its own doc type, status and source.

You can still change single rows after that.

## Saving

Nothing is written until you press **Apply**. The button shows how many rows you changed, for example `Apply (3)`. The changes are saved to the catalog and to the client's `_Vibe\docmeta.jsonl`, so a rebuild keeps them, and a message such as `Details set on 3 documents.` appears.

**Close** hides the panel without saving the rows you have not applied. You can still set the same details later in Browse.

> Changes made here can be taken back with Undo in Browse (see [undo](help:undo)). Undoing the batch itself is done on Intake's **History** screen.

## Large batches

The panel lists up to 300 documents. For a bigger batch, a line says how many more there are; set those in Browse (see [documents](help:documents)).
