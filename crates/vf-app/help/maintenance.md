# Rebuild, sweep and checking the cabinet

How Vibe Files keeps its catalog in step with the folders: the checks it runs when a cabinet opens, the integrity sweep that finds files changed outside the app, and rebuilding the catalog from the folder tree.

## Checks when the cabinet opens

Each time a cabinet opens, Vibe Files first puts back any change a crash cut short. Then it checks, in the background:

- **Catalog integrity**: the catalog file is not damaged.
- **Audit chain**: the [audit trail](help:audit) verifies.
- **Cabinet reachable**: the cabinet folder can be read and written.
- **Interrupted changes**: none were left half done.
- **Sync**: no conflicts are waiting and sync has run (when it is set up on this PC).

Anything wrong shows on Browse as "Startup check:" followed by what failed, for example "Sync: 2 conflicts to resolve (Sync tab)".

## The integrity sweep

People sometimes open a file from Explorer and save it, or move or delete files there. The sweep finds those changes. It checks every file in the client folders against the catalog: a file whose size or date changed is read in full, and so is one thirteenth of the others each week, so every file's content is re-checked at least once a quarter. A folder that cannot be read is listed and the sweep goes on. It runs on its own when the cabinet opens if the last sweep is 7 days old; open **Admin** › **Integrity sweep** and press **Sweep now** to run it at once.

The result lists:

- **Edited outside Vibe Files, kept as new versions**: handled for you. The new content becomes the document and the old one is kept as a version.
- **Moved outside Vibe Files**: press **Follow N moves** to update the catalog to the new places.
- **Missing**: press **Mark N as deleted**. Their rows stay as tombstones; **Undo last** takes it back.
- **Not in the catalog**: files that appeared in a client folder. Press **Adopt N where they are** to make them documents in place (then set their doc type in Browse), or **Send N to Intake** to file them through Intake's inbox.
- **Could not be read**: often a file open in another program. Close it and sweep again.

When nothing is wrong you see "Everything matches the catalog."

> On a shared cabinet, each PC learns about another PC's changes at its next sweep, or at once when both [sync](help:sync).

## Rebuilding the catalog

The catalog is an index: the files, the `_Vibe\client.json` in each client folder and the files in `_Vibe` hold what matters. Vibe Files rebuilds it on its own the first time a cabinet is opened on a PC; Browse then says "Indexed:" with the counts.

There is no rebuild button in the app. If the startup check says "the catalog is damaged … run `vibe-files rebuild`", ask whoever looks after Vibe Files. They close Vibe Files on that PC and run, from a command prompt, `start /wait vibe-files rebuild <cabinet folder>`. It is safe to repeat. The same checks as at start-up run with `start /wait vibe-files check <cabinet folder>`.

> With Vibe Files open on the same data folder, a command that changes things stops with "Vibe Files is already open with this data folder". Close the app first.

See also [where your data lives](help:data-and-backups).
