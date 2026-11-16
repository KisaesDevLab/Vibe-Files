# Bulk changes

How to find many documents by part of their file name and change, rename or trash them together, in **Admin** › **Bulk changes**.

## Finding the documents

1. Open **Admin** and click **Bulk changes** under **FIRM**.
2. In **File name contains**, type part of a file name, for example `GovtK1`.
3. Click **Find**, or press Enter.

Every document whose file name has that text is listed, capitals or not. Only the file name is looked at: a folder with the text in its name does not bring in its files. Up to 10,000 documents are listed. If there are more, the line above the grid says **First 10,000 shown**; make the text more exact, or work through them in several goes.

The grid shows each document's name, client, folder, doc type, year, status, source and size. Click a column's name to sort by it. Point at a name for its full path.

## Choosing rows

Tick the box at the start of a row. **Select all** ticks every row listed, **Select none** clears them, and the box in the grid's heading does the same. The line above the grid says how many are selected.

## Setting details

With rows ticked, choose a **DOC TYPE**, **TAX YEAR**, **STATUS** or **SOURCE**, or type **TAGS**. A field left at its current value is not changed. Click **Apply to N**.

See [Document details](help:documents) for what each field means.

## Renaming

1. Click **Rename…**.
2. In **Replace**, type the text to take out of the names; it starts as the text you searched for. In **with**, type what goes in its place, or leave it empty to take the text out.
3. Click **Preview**. Each document is shown with its new name. Nothing is renamed yet.
4. Click **Rename N**.

The extension is kept. A document whose name does not have the text is left as it is. If a new name is already taken in the folder, the document gets ` (2)` after its name, and the preview says so. A document that is checked out is not renamed.

## Moving to the trash

Click **Move N to trash**. A box names the count and the first few documents, and asks before anything is moved. The files leave their folders and can be restored from **Admin** › **Trash** for 90 days. See [Trash](help:trash).

## Large changes

A change to up to 500 documents is one transaction. A larger one is made 500 documents at a time. While it runs, a line shows how far it is, for example **1,500 of 4,200**, with **Cancel**. Cancel stops after the 500 being changed at that moment. What was already changed stays changed, and the note says how many that was.

On a network share each file is renamed or moved one at a time, so thousands take a while. The rest of Vibe Files can be used meanwhile.

## Undo

Each transaction is recorded in the audit trail. Right after a change, **Undo last** in Browse takes it back. A change made 500 at a time is taken back 500 at a time, the last ones first. See [Undoing changes](help:undo).

> Browse keeps the last 20 transactions for **Undo last**. A change to more than 10,000 documents cannot be taken back whole that way; documents moved to the trash can always be restored from **Admin** › **Trash**.
