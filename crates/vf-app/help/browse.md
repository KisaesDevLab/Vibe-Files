# Browsing the cabinet

The Browse tab: the client tree, the document grid, sorting and filtering, and the preview.

Browse has three panes: the tree on the left, the documents of what you picked in the middle, and the details of the selected documents on the right.


**Hide folders** (above the list, left) or **«** at the top of the folder tree collapses the tree; **Hide details** (right) or **»** at the top of the details pane collapses that pane, giving the list the room. **Show folders** and **Show details**, or the **»** and **«** left at the edge, bring them back. Drag the inner edge of either pane to make it wider or narrower. Click **VIEWS**, **FAVORITES** or **SAVED SEARCHES** to fold that section shut, and again to open it. This PC remembers all of these.
## The tree

The tree lists, from the top:

- **VIEWS** — ready-made lists across all clients: **Recent** (documents you opened or changed lately), **Pinned**, **Needs doc type**, **Needs tax year**, **Received this week** and **Folders with no client ID** (files in cabinet folders bound to no client; bind them with Admin › Folder import). Each shows a count.
- **FAVORITES** — clients, years or folders you added. Right-click one and choose **Remove from favorites** to drop it.
- **SAVED SEARCHES** — searches saved on the Search tab. See [Search](help:search).
- **CABINET** — **All documents**, then every client.

Type in **Find client by name or ID** to shorten the client list, for example `smith` or `1004`. When clients have partners, **Partner** at the top of the pane, above **VIEWS**, shows only one partner's clients, or **No partner**. It also keeps the Views (Recent, Pinned, Needs doc type, Needs tax year, Received this week) to those clients' documents, counts included. Intake's review filters by the same choice. Each Windows user's choice comes back next time.

### Folders or By year

Next to **CABINET** are two buttons:

- **Folders** — each client's folders exactly as they are on disk, such as `Income Tax` › `2025`.
- **By year** — client, then tax year, then folder. Handy when you want everything for 2025 at once.

Your choice is kept on this PC. The tree matches the folders on disk and is re-read every 30 seconds, so a folder made in Explorer appears on its own.

To add a client, year or folder to **FAVORITES**, right-click it and choose **Add to favorites**.

## The grid

Above the documents is the path of what you are looking at, for example `Cabinet / Baker Farms (1004) / Income Tax / 2025`, and the count of documents shown and selected.

The columns are **Name**, **Client**, **Doc type**, **Year**, **Status**, **Size**, **Modified**, **Filed by** and **Hash**. **Columns…** (or right-click a heading) hides or shows any but Name, for your Windows login. A document without a doc type shows a **Needs doc type** pill.

- Click a column heading to sort by it; click it again to reverse the order. A small arrow shows the sorted column.
- Type in **Filter this folder** to show only names containing that text.
- **Refresh** re-reads the catalog.
- Click the box at the start of a row, or use Ctrl and Shift with the mouse, to select several documents. See [Keyboard shortcuts](help:shortcuts).

Double-click a document, or press Enter, to open it: PDFs open in the [Viewer](help:viewer), other files in their own program. Right-click for **Open in Windows program**, **Open in Explorer**, **Copy path** and **Go to client's folder**, which opens the document's client in the tree on the left.

**Pin** lists the selected documents under **Pinned** on this PC; **Unpin** takes them off.

### Moving and copying documents

- **Drag** the selected documents onto a folder in the tree to move them there, also into another client's folder. Hold **Ctrl** as you drop to copy instead.
- **Right-click** › **Move to…** or **Copy to…** lets you pick a cabinet (this one, or **Another cabinet…** for a different Vibe Files cabinet), a client and one of its folders.

You confirm before anything changes. A name that is already taken keeps both files (` (2)`).

A copy is a new document with the original's doc type, year, status, source and tags. Moved into another cabinet, a document leaves this one; that cabinet gives it the same doc type and year when it next reads that folder. **Undo last** (Ctrl+Z) puts moves back and removes copies.

A file deleted or moved with Windows Explorer leaves Browse, or shows in its new place, when its client folder is next looked at. If it is put back from the Recycle Bin, it returns with its doc type and year.

## The preview

When one document is selected, the right pane shows its first page. **Previous** and **Next** step through the documents in the grid (as do the arrow keys), and **<** and **>** turn its pages. **Open in Viewer** opens the full Viewer at the page shown; **Open in program** opens the file in the program Windows has for it.

For a file that is not a PDF the pane shows what is inside it, read without Office: an Excel workbook, CSV or `.iif` file as a grid with a tab per sheet (the first 2,000 rows and 60 columns), a Word or PowerPoint file, RTF or text file as its text, a mail (`.eml`, Outlook `.msg`) with From, To, Date, Subject and the names of its attachments above its text, and a picture. It shows contents only, not the page layout, fonts or pictures inside a document. Older `.doc` and `.ppt` files and files over 40 MB are not shown: open them in their program.

Below the preview are the document's details. See [Document details](help:documents).

When a client is picked in the tree and no document is selected, the right pane shows the client instead. See [Clients](help:clients).

## See also

- [Moving, renaming and deleting](help:moving-renaming)
- [Adding files and new clients](help:new-files)
- [Scanning and printing from Browse](help:browse-scan-print)
