# The audit trail

What Vibe Files records about every change, how the hash chain shows tampering, how to check it, find rows and export them, and where the files are kept.

## What is recorded

Every change Vibe Files makes writes an audit row in the same step as the change itself: filing, moving, renaming, metadata edits, new versions, trash and restore, Intake batches, sync pushes and pulls, doc type and template edits, legal holds, disposition, admin password changes, every Admin unlock and every refused password. Prints are recorded too (see [printing](help:printing)).

Each row has a number, the time, the Windows login and PC that made the change, the action, the document and client it concerns, and a detail. Passwords and document contents are never written into it.

## The hash chain

Each row carries a fingerprint (SHA-256) of itself and of the row before it. Changing, deleting or inserting a row anywhere breaks the chain from that point, so tampering shows.

Vibe Files checks the whole chain when the cabinet opens and again every week it stays open. If it does not verify, a red banner on every tab says "The audit trail does not verify at row N: … Someone may have edited the catalog outside Vibe Files. See Admin › Audit trail." Tell whoever looks after Vibe Files.

## The Audit trail page

Open **Admin** › **Audit trail**. The top line shows the last check, for example "Chain verified …: 4210 rows, intact." Press **Verify now** to re-check every row now.

### Finding rows

Narrow the list with:

- **Client** (or **Any client**)
- **Action** (or **Any**)
- **User**
- **Document**: part of the path
- **From** and **To**: dates as `yyyy-mm-dd`

**Clear** removes the filters. The page shows the newest matching rows; exports always include every matching row.

### Exporting

- **Export CSV…** saves the matching rows as a spreadsheet file.
- **Export PDF…** saves them as a printable report titled "Vibe Files audit trail", for example for a peer review or an examiner.

Times in the exports are in UTC.

## Where the files are

Besides the catalog, every row is written to a monthly file in the cabinet: `_Vibe\audit\yyyy-mm.jsonl` (for example `_Vibe\audit\2026-11.jsonl`). These are plain text, one row per line, and are sent to the bucket by [sync](help:sync), so they survive the loss of a PC.

> Do not open or edit these files in another program. Any change breaks the chain.

See also [rebuild, sweep and checks](help:maintenance).
