# Conflicts and restoring

What happens when the same document changes on two PCs, how to choose which copy to keep, how deletions travel, and how to get files back from the bucket.

## How a conflict happens

A pull compares each document three ways: what the bucket has, what this PC has, and what the two last agreed on.

- Changed on another PC only: the new content comes down. The old content is kept as a version.
- Changed on this PC only: it waits for the next push.
- Changed on both: nothing is overwritten. The bucket's content keeps the document's name, and this PC's content is kept beside it as a copy named like `Engagement letter (conflict from FRONT-DESK).pdf`, where `FRONT-DESK` is this PC's name. A conflict is raised.
- Deleted on another PC but changed here: this PC's copy is kept, and a conflict is raised.

The Sync tab's **Status** shows the number of conflicts, and the startup check on Browse reminds you while any are open.

## The Conflicts list

When there are conflicts, the Sync tab shows **Conflicts** with a count. Each line names the document and says what happened.

For a document changed in both places:

- **Keep mine**: this PC's content becomes the document's content (the other is kept as a version). The conflict copy goes to the trash.
- **Keep theirs**: the other PC's content stays. The conflict copy goes to the trash.
- **Keep both**: both files stay as they are, as two documents.

For a document deleted on another PC but changed here, press **OK**: this PC's copy stays.

Your choice goes up with the next push. You can open both files in the Viewer first to compare them.

## Deletions

A document moved to the trash on one PC stays listed in the bucket as deleted (a tombstone); its content is not removed from the bucket. On the next pull the other PCs move it to their own trash, where it can be restored like any other deleted document (see [trash and legal holds](help:trash)).

## Restoring from the bucket

On a PC where sync is unlocked:

- **Restore cabinet to a folder…** (admin password needed) rebuilds the whole cabinet from the bucket into an empty folder you choose. Every file comes back checked against its SHA-256, with client folders and their `_Vibe\client.json`, versions, doc types, years and the audit trail. Then open that folder as a cabinet.
- In **Pending by client**, a client's **Restore to a folder…** writes that client's current documents as plain files into a folder you choose.

> When the server or the cabinet folder is lost, a bare PC can also restore from the command line with the bucket's details and the sync passphrase: `vibe-files restore`. Ask whoever looks after Vibe Files.

See also [syncing to B2](help:sync) and [the sync key](help:sync-key).
