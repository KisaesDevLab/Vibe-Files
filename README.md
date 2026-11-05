# Vibe Files

Document management for small CPA and tax firms, on Windows. Vibe Files works over a plain
client folder tree: your files stay ordinary PDFs and Office files in ordinary Windows folders,
and Vibe Files adds browsing, intake, OCR, full-text search, PDF tools (annotate, fill forms,
page tools), versions, an audit trail, retention and legal hold, and optional two-way sync to
your own Backblaze B2 bucket.

Its client folder tree and document-filing engine come from
[Vibe Filer Desktop](https://github.com/KisaesDevLab/Vibe-Filer-Desktop), which runs inside
Vibe Files as the **Intake** tab.

> **Pre-release.** In use at Kisaes LLC for its live run. Releases are on the
> [Releases page](https://github.com/KisaesDevLab/Vibe-Files/releases).

## Install

Windows 10 or 11, 64-bit. Everything Vibe Files needs is included: PDF viewing (pdfium),
scanning (TWAIN), and OCR (Tesseract with English).

### With winget (available soon)

Vibe Files is being added to the Windows Package Manager. Once it is listed:

```powershell
winget install Kisaes.VibeFiles      # install
winget upgrade Kisaes.VibeFiles      # update to the newest release
```

Start it with `vibe-files` or from the Start menu. Its data folder is
`%LOCALAPPDATA%\Vibe Files\data`; upgrades never touch it.

### From the release zip (available now)

1. Download `vibe-files-<version>-win64.zip` from the
   [latest release](https://github.com/KisaesDevLab/Vibe-Files/releases).
2. Check the download. In PowerShell:
   `Get-FileHash vibe-files-<version>-win64.zip`
   must show the SHA-256 given in the release notes (and in `SHA256SUMS.txt`).
3. Unzip it into a folder of your choice, for example `C:\Vibe Files`, and run
   `vibe-files.exe`.

The data folder (`data\`) is created beside `vibe-files.exe`. To upgrade, replace everything in
the folder except `data\`.

## First setup

Vibe Files needs a **cabinet**: the folder that holds your client folders, on this PC
(`C:\Clients`) or on a share (`\\server\share\Clients`).

1. Start Vibe Files. The **Cabinet** screen opens.
2. Click **Create new**, choose the **Cabinet folder** with **Browse…**, type a **Cabinet name**
   (shown in the header), and click **Create cabinet**.
   - The folder may already hold client folders. They are indexed where they are; nothing is
     moved.
3. On the other PCs in the office, click **Open existing** instead and choose the same folder.

Vibe Files remembers the cabinet and opens it on its own the next time.

## Bring in your clients and an existing client folder tree

Do these in order.

1. **Import the client list.** Export the client list from your tax software as CSV. In
   **Browse**, click **Import clients CSV…**, choose the file and click **Read**. Pick the
   **Client ID column** and the **Name column**. For a taxpayer-ID column, tick **keep the last 4
   digits only** (full SSNs and EINs are never copied in). Click **Import N rows**.
   - This only saves the list. No folder is renamed, moved or tied to a client yet.
   - Until step 2, Browse shows these clients as **No folder yet**. Don't click
     **Create folder…** for them, or you get a second, empty folder beside the old one.
2. **Scan the existing folders.** Open **Admin › Folder import** and click **Scan the cabinet**.
   Each top-level folder is matched to a client by its ID or its name, and each file gets a doc
   type and tax year where its name or folder says so.
3. **Check the matches.** The **Match** column shows how sure each match is: 90–100% are ticked
   for you, 60–84% need a look, 50% or less mean something is off. To change a folder's
   client, pick another in its **Client** column. No client list? **Create a client for each
   unmatched folder** makes a client from each folder's name, with an ID you can change.
   Files with no doc type found can all be given one doc type and source.
4. **Tick the folders to import.** **Show matches from N% up** filters the list; **Tick all
   shown**, **Untick all shown**, the **Bind** box at the top of the column, and Shift-click
   tick or untick many at once.
5. **Optionally rename the folders** to the firm's naming, `{Client} ({ID})`, for example
   `Smith, John` → `Smith, John (SMIT1001)`: tick **Rename bound folders to the firm template's
   name**. Files inside are not moved.
6. Click **Import N folders**. **Undo the last import** reverses it.

### A very large old tree: move the active files first (vibe-move)

If the old client tree is huge, you can bring only the files you still work with into Vibe
Files and leave the rest as an archive. **vibe-move.exe**, beside vibe-files.exe, does this
before step 2. It works on plain folders, and Vibe Files doesn't need to be open.

1. Choose the **Old folder** (it becomes the archive) and a **New folder** (your active
   cabinet), then click **Scan the old folder**. Every client folder is ticked; untick any to
   leave out.
   **Clients per run** moves only that many clients' files at a time (blank = all), to try
   the process on a few first; Preview again after a move for the next ones.
2. Run each of the three steps, in any order. Each shows a **Preview** of what would move
   before you confirm:
   - **Permanent folders**: each client's top-level folder with the name you type (default
     `Permanent`) moves whole.
   - **Recently modified**: files modified on or after a date.
   - **Year in the name**: files with that year or later in the file name or in a folder
     inside the client folder (`Taxes\2024\…`).
3. Files keep their paths under the new folder. Moves between drives are copied, checked and
   only then removed from the old folder. A name already taken keeps both files (` (2)`).
   A file open in another program is tried again for about 10 seconds, then left and listed.
   Folders a move empties are removed. Every move is written to a CSV log beside the new folder.
   **Undo this move** (or **Undo a move from its log…** later) puts a step's files back,
   except any changed since the move.

Then import the **new** folder as your cabinet (steps 1–6 above).

New clients after that: **+ New client** in Browse creates the client's folder from the firm's
folder template. To fix a client's name later, use **Rename client…** in the client panel; its
folder is renamed to match.

## Day to day

- **Help** (top right in the app) covers every screen: browsing, Intake rules, scanning, the
  Viewer, search, versions and undo, retention, sync, and troubleshooting.
- `OPERATIONS.md` in the release zip is for whoever sets up and looks after the office's PCs:
  several PCs on one cabinet, backups, B2 sync, and upgrades.

## Privacy

- No telemetry. The only network calls go to your own B2 bucket, when you turn sync on.
- Files sent to B2 are encrypted on your PC first, with a key only your firm holds.
- Full SSNs and EINs are never stored in the client list or the catalog; only the last four
  digits are kept.

## License

[GNU Affero General Public License v3.0 or later](LICENSE) (`AGPL-3.0-or-later`).
Third-party components keep their own licenses: pdfium (in `crates/filer-pdf/assets/`), the TWAIN
Data Source Manager (LGPL-2.1-or-later, in `crates/vf-ingest/assets/`) and Tesseract
(Apache-2.0, in the release's `tesseract\` folder).
