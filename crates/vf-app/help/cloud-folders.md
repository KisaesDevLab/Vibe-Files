# Cabinets in OneDrive or Dropbox

A cabinet can be kept in a OneDrive or Dropbox folder that every PC syncs. Vibe Files notices this and works so that no PC downloads the whole cabinet in the background, and so that its catalog is not damaged by syncing.

## Online-only files

OneDrive and Dropbox can keep a file "online only": it shows in the folder, but its contents are downloaded the moment anything reads it. Vibe Files does not read such a file in the background:

- **Indexing** keeps an online-only file as the catalog has it, never as deleted or changed. A new one that another PC filed takes its details from that PC's notes in the client's `_Vibe\docmeta.jsonl` without being read. One added outside Vibe Files is listed once it is on this PC.
- **The weekly check** counts online-only files as not checked; they are checked once they are on this PC.
- **Text recognition** sets them aside and reads them once they are on the PC that runs it.
- **Search results** show no thumbnail for them, and **Browse's preview** shows **Show preview** instead of the page: pressing it downloads the file.

Opening a document (double-click, the Viewer, Open in program) downloads it, as it would in Explorer. To keep a client's files on a PC, right-click its folder in Explorer and choose **Always keep on this device** (OneDrive) or **Make available offline** (Dropbox).

## Each PC keeps its own catalog

The catalog is a database that a sync service would damage while it is open, so it never lives in the cloud folder:

- Each PC keeps its catalog in `ProgramData\Vibe Files\catalogs`. The first PC indexes the cabinet and leaves a copy in the cabinet's `_Vibe\catalog-seed` folder (refreshed weekly); the others start from that copy, with every document's details and recognized text, so they read nothing again.
- Each PC notes its changes in its own log in `_Vibe\changes`; the others read them when a client is opened and every 15 minutes.
- Text recognition and B2 sync run on one main PC, chosen in **Admin** › **Devices** with **Run them on this PC**. In a cloud cabinet no PC takes this on by itself (two could, before the folder synced), so Browse reminds you until one is chosen. Only the main PC leaves the catalog copy.
- A data folder found inside OneDrive or Dropbox (Vibe Files unzipped on a Desktop that OneDrive syncs, for example) is moved to `%LOCALAPPDATA%\Vibe Files\data` with its catalog and settings, and a `MOVED.txt` note is left behind.

## The setting

**Admin** › **Devices** › **Cloud folders** › **Cloud folder handling on this computer**:

- **Automatic** (the default): on when Windows or Dropbox reports the cabinet's folder as theirs.
- **Always on**: for a folder Windows does not report, such as another sync service.
- **Always off**: every file is read as on an ordinary disk, so each online-only file is downloaded.

A change takes effect when Vibe Files next starts on that computer. Remote Desktop and Citrix servers keep their own **Server mode** setting; a cabinet on a file share is not a cloud folder.

## Not done (yet)

- **Search text of new scans on other PCs.** Text recognition runs on the main PC and writes the text into the PDF. Another PC finds that text once the file is on it; while it is online only there, the scan is found by its name and details, not its words. Text known when the PC started from the catalog copy is searchable at once.
- **Thumbnails** are made by each PC from the file itself, so online-only files have none.
- **Keeping a client on this PC** is done in Explorer (above), not in Vibe Files.
- **The audit trail and each client's `docmeta.jsonl`** are still one shared file each, so two PCs writing to the same one between syncs leave a conflicted copy (below).

## Things to know

- **Use B2 sync or a cloud folder, not both.** B2 sync reads every file to send it, so the PC that runs it downloads the whole cabinet. The Sync tab says so.
- **Not on Remote Desktop servers.** On a server each person logged in would sync their own copy of the cabinet. Keep a server's cabinet on a file share; **Admin** › **Devices** warns when it is not.
- **Conflicted copies.** OneDrive and Dropbox do not merge two PCs' changes to the same file: they keep both, adding "conflicted copy" (Dropbox) or the PC's name (OneDrive) to one. When this happens to one of Vibe Files' own files (the audit trail, a client's `docmeta.jsonl`), Browse says so when the cabinet opens. Tell your administrator: the two need to be put back together by hand. It is most likely when two PCs file documents for the same client within a minute or two.
- **Files are not encrypted** in OneDrive or Dropbox, unlike B2 sync, which encrypts every file before it leaves the PC. Consider this for your §7216 and security plan obligations.
