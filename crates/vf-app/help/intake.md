# Filing documents with Intake

How the Intake tab files the PDFs that land in your inbox folders: reading the queue, fixing a row, committing a batch, undoing it, and what Vibe Files adds on top.

## A client that is not in the list

When a row says its client was not found, **New client…** beside **Choose client…** (also at the bottom of the client picker) opens the New client dialog with the name and ID from the file name. Check them, choose **Create client (one transaction)**, and the row is given the new client: its folder is made from the folder template, as from Browse. See [adding clients](help:new-files).

## Looking at a file, and fixing several rows at once

**Preview pane** in the queue's toolbar shows the pages of the file whose name you last clicked, beside the list, with **Previous** and **Next** (it opens by itself on a wide screen). Tick rows to fix them together: **Set client…** gives every ticked file one client, **Apply rule…** files every ticked file by the rule you choose, and **Untick all** clears the ticks.

## The queue

The **Intake** tab opens on **Preview**. Every PDF in an inbox folder (set in **Setup**, see [rules and Setup](help:intake-rules)) shows as one row. Intake reads the client ID from the file name, picks a rule, and works out where the file will go. Each row has these columns:

- **File**: the file name. Right-click it for the row menu.
- **Date**: when the file was last changed (point at it for the time).
- **Client**: the client the ID matched, for example `Smith, John (SMIT1001)`. A `fuzzy` pill with a score means it matched by name, not ID: check it.
- **Rule**: the rule that sets the folder and name. Click it to choose another rule.
- **Year**: the tax year. Amber when missing.
- **Destination**: the folder and file name it will get.
- **Status**: **Ready**, **Needs client**, **Needs year**, **Collision**, **Skipped**, **Paused · duplicate folder** or **Error**. Extra pills say **Will create** (a new client folder) or **Cross-drive**.

Click a column heading to sort by it; again to reverse. **Date** sorts newest first; empty values go last.

The toolbar has **Partner** (only that partner's clients' rows are listed and committed; the same choice as Browse's), **Batch year** (for rows without a year), **On collision** and **Keep all original names**.

**Tick ready** ticks every row whose status is **Ready**; the other rows stay unticked.

## Fixing a row

- No client: click the suggested client, or **Choose client…**.
- Wrong client: right-click › **Change client…**.
- No year: pick one in the **Year** box, or set **Batch year**.
- A row's rule name, and **Apply rule…** for ticked rows, open a list of rules with a **Type to search** box.
- Right-click also offers **Move to trash**, which takes the file out of the inbox into the cabinet's [trash](help:trash) (with rows ticked, **Move to trash** beside them does it for all of them); **Restore** there puts it back in the inbox. It also offers **Skip row** (Del does the same), **Split…** for a stack of several documents, **Rename…**, **Change folder…** and **Copy to another client…**. **Use the rule's name** and **Use the rule's folder** undo your own choice.
- To merge files, tick two or more rows of the same client and press **Combine** in the toolbar.

## Commit

When every row is Ready, Skipped or Paused, press the Commit button at the bottom (it says how many, for example `Commit 8 files`). Enter does the same. A summary lists the moves, copies, new folders and overwrites before anything is written; press its Commit button to file, or **Cancel**. If you tick some rows, only those are committed.

## Undo last batch

Open **History** and press **Undo last batch** (Ctrl+Z). The files go back to their inbox. It refuses if a filed file was changed or moved since. Only this PC's last batch can be undone, and only until its next batch.

## What Vibe Files adds

- Each filed file becomes a document in the catalog, with an audit entry. Browse shows it at once.
- Files that came from a [cover sheet](help:cover-sheets) or [print capture](help:print-capture) arrive with their doc type, tax year and source already set. Others get the year from their folder and no doc type: set them in the [Just filed](help:just-filed) panel that opens after each batch.
- **Tax forms are read.** A file whose name no Intake rule names is read for its form: W-2, 1099 (INT, DIV, B, R, NEC, MISC), 1098, SSA-1099 and K-1, including the values typed into a fillable PDF and scans (through OCR). That covers files in the inbox and scans or print captures with no client in their name. The form gives the doc type and tax year, and the recipient's name and the last four digits of their tax ID propose the client. On the row, a **read** pill says what was found (hover it); the client and year are filled in only where the row has none, and nothing you chose is changed. You still confirm the row. The filed document gets the doc type and year and is tagged `import:guessed` so someone checks it: it is not disposed of until its doc type or year is set by hand, which removes the tag.
- **Already in the cabinet**: a filed file with the same content as an existing document. Choose **Remove new copy**, **Keep both** or **Replace old**. Removed copies go to the [trash](help:trash).
- New clients and relationships reach Intake on their own; what you chose on rows stays.
- A banner with **Reload Intake** appears when Intake's settings, folder template or the print capture folders change. Reloading reads the inbox again: changes you made on rows but did not commit are lost.

> Several people can file at the same time. See [Several people filing at once](help:several-pcs).
