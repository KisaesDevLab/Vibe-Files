# Undoing changes

Taking back your last changes with **Undo last** on Browse (Viewer edits included), what can and cannot be undone, and how Intake's own undo differs.

## Undo last

Every change Vibe Files makes to documents is one transaction, so it can be taken back as a whole: moving 12 documents is one change, and one undo puts all 12 back.

To undo your newest change:

- click **Undo last** in the right-hand pane of Browse, or
- press Ctrl and Z on Browse (when you are not typing in a box).

Hold the mouse over **Undo last** to see what it will undo, for example **Undo move** followed by the documents it covers and **(Ctrl+Z)**. When it is done, a note starting **Undid** says what was undone. When there is nothing to undo, the button is greyed out and says **Nothing to undo** on hover.

## History

Next to **Undo last** is **History** with a count, such as **History (5)**. It lists your last changes, newest first, with the time. Only the newest can be undone (the **Undo:** line at the top); undo it, and the one before it becomes the newest.

The list holds the last 20 changes made on this PC. Changes made on another PC are not on your list.

## What you can undo

- Metadata edits (doc type, tax year, status, source, tags)
- Renames and moves
- Moves to the trash, and restores from the trash
- Restoring an earlier version
- Annotations, stamps, page edits, redaction, flattening, PDF/A conversion and compression in the Viewer
- Combining PDFs and scanning into a folder from Browse
- A new client

Undo is refused, with **Undo failed:** and the reason, when undoing would lose work: for example if a file was changed or removed since. Only folders that are empty are removed, and files are never overwritten.

## What is not on the undo list

- **Intake batches.** Intake has its own undo, see below.
- **Folder import.** It has its own undo in Admin › **Folder import**.
- Work done in the background: text reading (OCR), rebuilding the catalog, B2 sync, and files copied in with Explorer that Browse picked up.
- Firm settings: doc types, retention classes, folder templates and the client list.
- Legal holds and disposition.
- Emptying the trash: those files are gone for good.
- Printing, which is only recorded in the audit trail.

## Intake's Undo last batch

Intake files a whole batch at a time and keeps its own undo. On the Intake tab, open **History** and click **Undo last batch** (or press Ctrl and Z while on Intake). It asks before it undoes.

Intake's undo and Browse's **Undo last** are separate: Ctrl and Z on Browse never undoes an Intake batch, and Ctrl and Z on Intake never undoes a Browse change. When Intake's batch is undone, Browse shows **Intake's last batch was undone.**

See [Intake](help:intake) and [Just filed](help:just-filed).

## Undo and the audit trail

Undoing does not erase anything from the audit trail: the change and its undo are both recorded. See [Audit trail](help:audit).
