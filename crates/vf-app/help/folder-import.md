# Importing an existing folder tree

How to adopt client folders that are already in the cabinet (an older firm tree, often without client IDs in the folder names): matching each folder to a client, checking the matches, filling in doc types and years, and undoing it.

## Before you start

Import the client list first if you have one (Browse › **Import clients CSV…**, see [Clients and relationships](help:clients)). This only saves the list: no folder is renamed, moved or tied to a client yet. Until the folder import, Browse shows these clients as **No folder yet**; do not press **Create folder…** for them, or you get a second, empty folder beside the old one.

No client list? You can still import: the folders can become clients of their own (see below).

A very large old tree? **vibe-move.exe**, in the same folder as Vibe Files, can first move only the files you still work with into a new folder, and leave the rest as an archive:

- each client's **Permanent** folder;
- files modified on or after a date;
- files with a recent year in their name or folders;
- **Year folders**: folders whose name has a year in a range you type (`2026`, `2023-2025`), such as `Income Tax\2026` or `2026 Returns`, moved whole with their sub-folders.

Each step shows a preview first, and every move is logged in a CSV file beside the new folder. A file whose new path would be too long (over 240 characters) gets a shorter name, cut in the middle and marked `~`, unless you untick that. **Undo this move**, or **Undo a move from its log…** later, puts a step's files back; a file changed since the move is left. Then import the new folder.

## Scan the cabinet

Open **Admin** › **Folder import** and press **Scan the cabinet**. Nothing is changed by the scan. The folders show first, each matched to a client by its ID or name, while the files are read behind (the page counts them; **Stop** ends it, **Read the files** starts again). Import waits for the files. The scan, with your ticks, is kept on this PC until you import or scan again. Each file gets:

- a **tax year** from a year folder it is in (`Income Tax/2022`), or else from a year in its file name (`…_2022_…`). A year from the file name is flagged to check;
- a **doc type** from its name or folder, where they say so.

## Check the matches

**Match** shows how well each folder matches its client:

- **90–100%**: the ID and the name agree, or the same name of two words or more. These are ticked for you.
- **60–84%**: the same one-word name, or only a similar name. Check these, then tick them.
- **50% or less**: two folders point at the same client, several clients have the folder's name, the ID is the client's but the name is not, or the client already has a folder.

To change a folder's client, pick another in its **Client** column (type to search).

**Show matches from N% up** lists only folders matched at least that well; **Hide already bound** and **Hide folders with no match** shorten the list. **Tick all shown** and **Untick all shown**, or the **Bind** box at the top of the column, tick or untick every folder listed. Click one folder's box, then Shift-click another to tick or untick all the folders between.

## Folders that match no client

With no client list imported, the page says so. **Create a client for each unmatched folder** gives each such folder a new client: the folder name becomes the client name, and an ID is made from it (`1 Farm Poultry LLC` gets `1FAR1001`). Change an ID in the box beside the client on its row, then press Enter. The clients are added to the client list when you import. A client list imported later keeps them when it has the same IDs.

## Files with no doc type

When some files have no doc type found, the line **N files with no doc type found get:** offers a **Doc type** and a **Source** for all of them. They are applied when you import and flagged to check: such a document is not disposed of until its doc type or year is set by hand.

## Import

- To add the client ID to the folder names, tick **Rename bound folders to the firm template's name**. For example, `Smith, John` becomes `Smith, John (SMIT1001)`. Left unticked, folders keep their names and are still tied to their clients. Files inside are never moved.
- Press **Import N folders**. Ticked folders the filter hides are imported too; the button then says how many, for example **Import 12 folders (3 not shown)**.
- **Export the remainder (CSV)…** lists files still missing a client, doc type or year, for a person to handle.
- **Read the text of the imported PDFs (OCR)** queues the imported files for [text recognition](help:ocr). It is ticked by default up to 50,000 files. Left unticked, text recognition reads only documents filed from now on; the imported files are still found by name and details.
- **Undo the last import** removes the ties and puts the old folder names back.

## Large cabinets: batches, stopping and continuing

One press of **Import** imports every ticked folder, 250 at a time, going on by itself. Each batch ties its folders to clients, catalogs their files and sets doc types and years. The page shows the folders done and the batch's files read.

Each batch is saved when it finishes. If the import is cut off (Vibe Files closed, the PC restarted), nothing finished is lost:

- Open **Folder import** again and press **Scan the cabinet**. Folders from finished batches show as already bound.
- Tick the rest and press **Import** to carry on from there.

**Undo the last import** undoes every finished batch of that import.
