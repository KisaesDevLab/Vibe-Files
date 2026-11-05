# Merging cabinets

How to move another Vibe Files cabinet's client folders into this one: what is matched, what happens to files that are already here, and how to undo it.

## Before you start

The other cabinet must be a Vibe Files cabinet (it has a `_Vibe` folder), separate from this one: neither may be inside the other. Its client folders need their client sentinel: a folder without one is not a client folder and stays where it is; adopt it there first with [Folder import](help:folder-import), or move it by hand. Make sure nobody is working in the other cabinet while you merge.

## Scan

Open **Admin** › **Merge cabinets**, choose the other cabinet with **Browse…** (or type its folder) and press **Scan**. Nothing is changed by the scan. Each of its client folders is listed with:

- **Client here**: the client in this cabinet with the same client ID, and its folder. The same ID is the same client. An ID this cabinet does not have becomes a **new client**, with its name from the other cabinet and a folder made from this cabinet's template.
- **Files**: how many files it holds.
- **Already here**: how many of them have a file at the same place in this cabinet.

Untick a client to leave it out; **Tick all** and **Untick all** change every row.

## Merge

Press **Merge N clients**, then **Yes, merge**. For each client:

- Each file moves to the same sub-folder of the client's folder here (`Income Tax/2022/…`), checked by its fingerprint as it moves. It keeps its doc type, tax year and tags.
- A different file already at that place stays, and the incoming one gets ` (2)` beside it.
- The same file already here: the other cabinet's copy is left where it is, and counted.
- Folders left empty in the other cabinet are removed.

The other cabinet's audit trail is copied into this cabinet's `_Vibe\audit\merged\` folder as history. Earlier versions and the other cabinet's client details (entity types, relationships, partners) are not brought over.

## Undo

**Undo the last merge** moves every file back to the other cabinet and removes their documents here. New clients and their (empty) folders stay.
