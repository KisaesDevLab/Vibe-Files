# Moving, renaming and deleting documents

How to move documents to another folder, rename them from a template, combine PDFs, and send documents to the trash, all from Browse.

## Moving documents

1. In the grid, select the documents to move.
2. Drag them onto a folder or client in the tree on the left. If you drag a row that is not selected, only that row moves.
3. A dialog asks **Move 3 documents?** and shows the destination, for example **To Baker Farms (1004) / Income Tax / 2025**.
4. Click **Move**, or **Cancel**.

You can drop on a folder in either tree view, **Folders** or **By year**. In **By year**, dropping on `2025` › `Income Tax` moves the files into the `Income Tax/2025` folder.

The dialog warns you before anything happens:

- If a file with the same name is already there, both are kept: the moved one gets ` (2)` added to its name. Existing files are never overwritten.
- Files that cannot be moved are listed with the reason after **Not moved:**.
- If every file is already in that folder, you see **Already in that folder.**

After the move, **Undo last** (or Ctrl and Z) puts the files back. See [Undoing changes](help:undo).

## Renaming documents

1. Select one or more documents.
2. Click **Rename…** above the grid.
3. In **File name template**, type the new name using tokens, for example `{Client} {Year} {DocType}`. The tokens are `{Client}`, `{ID}`, `{Year}`, `{DocType}`, `{Rest}`, `{Original}` and `{Ext}`, plus the columns of your client list.
4. The table shows each file **NOW** and its **NEW NAME**, with notes such as **name taken: keep both**, **no change** or **skipped:** and the reason.
5. Click **Rename 3 files (one transaction)**.

Files stay in their folders. A name that is taken gets ` (2)`.

## Combining PDFs

Select two or more PDFs and click **Combine**. You get one new PDF of them, in the order shown in the grid, with a bookmark for each document. It is named after the first one, such as `Combined 2025 W-2.pdf`, saved in that document's folder and tagged `combined`. The originals are left as they are.

## Deleting (move to trash)

- Right-click a document and choose **Move to trash**, or
- select documents and click **Move 3 to trash** in the details pane.

One document goes straight to the trash. For two or more, Vibe Files asks **Move 3 documents to the trash?**; click **Yes, move 3 to trash** or **Cancel**.

Deleted files are kept in the cabinet's trash folder, not erased.

## Getting deleted documents back

- Right away: **Undo last** (or Ctrl and Z) puts them back where they were.
- Later: open **Admin**, choose **Trash**, tick the documents, and click **Restore** (it shows the count, such as **Restore 2**). The trash keeps documents for 90 days, then empties them on its own (documents under a legal hold stay).

See [Trash](help:trash).

## Working in Explorer

The files are plain files, so you can also move or rename them in Explorer. Vibe Files does not follow such a move on its own: the **Integrity sweep** in Admin finds it and follows it in one click. Moving in Vibe Files is better: the catalog stays right, the move is in the audit trail, and you can undo it. See [Maintenance](help:maintenance).

## See also

- [Browsing the cabinet](help:browse)
- [Document details](help:documents)
- [Audit trail](help:audit)
