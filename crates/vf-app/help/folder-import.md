# Importing an existing folder tree

How to adopt client folders that are already in the cabinet (an older firm tree, often without client IDs in the folder names): matching each folder to a client, checking the matches, filling in doc types and years, and undoing it.

## Before you start

Import the client list first if you have one (Browse › **Import clients CSV…**, see [Clients and relationships](help:clients)). This only saves the list: no folder is renamed, moved or tied to a client yet. Until the folder import, Browse shows these clients as **No folder yet**; do not press **Create folder…** for them, or you get a second, empty folder beside the old one.

No client list? You can still import: the folders can become clients of their own (see below).

## Scan the cabinet

Open **Admin** › **Folder import** and press **Scan the cabinet**. Nothing is changed by the scan. Each top-level folder is matched to a client by its ID or its name, and each file gets:

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
- **Undo the last import** removes the ties and puts the old folder names back.

## Large cabinets: batches, stopping and continuing

The import works through the folders 500 at a time. Each batch ties its folders to their clients, catalogs only those folders' files, and sets their doc types and years. While it runs, the page shows how many folders are done, for example **Importing: 1500 of 7628 folders done**.

Each batch is saved when it finishes. If Vibe Files is closed or the PC restarts partway through, nothing finished is lost:

- Open **Folder import** again and press **Scan the cabinet**. Folders from finished batches show as already bound.
- Tick the rest and press **Import** to carry on from there.

**Undo the last import** undoes every finished batch of that import.
