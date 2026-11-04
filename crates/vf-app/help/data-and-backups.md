# Where your data lives

What is kept in the cabinet folder, what is kept in each PC's data folder, why the catalog can always be rebuilt, and how to keep backups.

## The cabinet folder

The cabinet is an ordinary Windows folder, usually on the office file server (for example `\\server\Clients`). It holds one folder per client, such as `Smith, John (SMIT1001)`, with the sub-folders of the [folder template](help:folder-templates). Every document is a plain file: any PDF or Office file opens in Explorer without Vibe Files.

Each client folder has a small `_Vibe` folder holding `client.json`, which ties the folder to its client, and the documents' doc type, year and tags.

The cabinet's own `_Vibe` folder holds what the firm shares: the client list and relationships, doc types, the folder template, retention classes, legal holds, the admin password's hash, Intake's settings and rules, earlier versions of documents (`_Vibe\versions`), the trash (`_Vibe\trash`), the audit trail's monthly files (`_Vibe\audit`) and the sync key files (`_Vibe\keys`).

> Do not edit or delete files in `_Vibe` by hand. The one exception is resetting a forgotten [admin password](help:admin-password).

## The data folder on each PC

Each PC keeps its own data folder, on its local disk:

- beside `vibe-files.exe`, in `data\`, for a copy unzipped into a folder;
- in `%LOCALAPPDATA%\Vibe Files\data` for a winget install, and for every Windows login other than the first on a shared PC.

It holds:

- `catalog.db`: this PC's catalog of the cabinet. It is plain SQLite and holds client names and the text of every document, SSNs included. Protect it like the cabinet share: only firm staff, on an encrypted disk.
- `config.toml`: this PC's settings and its device id (see [devices](help:devices)).
- a remembered sync key, if you ticked **Remember on this PC for my Windows login**.
- caches (thumbnails, OCR) and `logs`, kept 30 days. The logs record actions and counts only: no names, paths or document contents.

> Never put the data folder on the network share. The catalog is not safe to open over the network.

## The catalog can be rebuilt

The catalog is an index of the cabinet. The files, each client folder's `_Vibe\client.json` and the cabinet's `_Vibe` folder hold what matters, so a lost or damaged catalog can be rebuilt from the folder tree. A new PC builds its own the first time it opens the cabinet. See [rebuild, sweep and checks](help:maintenance).

## Backups

- **The cabinet folder** is what to back up: include `_Vibe` and every client folder. Back it up the way the firm backs up the file server.
- **Backblaze B2 sync** keeps an encrypted offsite copy, with versions and the audit trail; the whole cabinet can be restored from it with the sync passphrase. See [syncing to B2](help:sync) and [conflicts and restoring](help:sync-conflicts).
- **The data folder** need not be backed up: the catalog is rebuilt from the cabinet, and sync is set up again with the bucket details and the sync passphrase. Keep the passphrase with the firm's recovery papers.
