# Vibe Files — operations

For the person who sets up and looks after Vibe Files in an office (v1: Kisaes LLC). Remote
laptops (remote mode) arrive with PA-17 and are not covered here.

## 1. Install on a workstation

Two ways; both give the same program.

- **winget** (Windows 10/11): `winget install Kisaes.VibeFiles`, later
  `winget upgrade Kisaes.VibeFiles`. Start it with `vibe-files` or from the Start menu search.
  Its data folder is `%LOCALAPPDATA%\Vibe Files\data` (winget replaces its own program folder on
  upgrade, so nothing of yours lives there).
- **Portable copy**: unzip `vibe-files-<version>-win64.zip` from the GitHub release
  (https://github.com/KisaesDevLab/Vibe-Files/releases) into a folder you choose, e.g.
  `C:\Vibe Files`. Check the zip against the release's `SHA256SUMS.txt` first:
  `Get-FileHash vibe-files-<version>-win64.zip` must show the same SHA-256. The data folder is
  `data\` beside `vibe-files.exe`. To upgrade, replace everything but `data\`.

**One data folder per Windows login.** On a server where several people run the app in their
own sessions, `data\` beside the exe belongs to the first login that used it (named in
`data\owner.txt`); every other login gets
`%LOCALAPPDATA%\Vibe Files\data`. A data folder is used by one running copy at a time: a second
copy for the same folder says who has it open (from `in-use.txt`) and quits (exit code 7 from
the command line). The lock is `<data>\in-use.lock`; it is released when the app ends, crash included.

The release carries `pdfium.dll` (PDF viewing), `TWAINDSM.dll` (TWAIN scanning) and a `tesseract\`
folder (OCR, English); nothing else needs installing. No installer, no registry writes (the
Explorer context-menu entry is opt-in).

`<data>\` holds the catalog (`catalog.db`, plain SQLite, PA-18: it contains client names and the
text of every document, SSNs included), `config.toml`, caches and logs. Protect it like the
cabinet share: only firm staff, on a disk with BitLocker.

The app checks GitHub once per start for a newer release and says so on Browse; it never
downloads or installs anything. `update_check = false` in `config.toml` turns that off.

## 2. A new office

1. Choose the cabinet folder: one folder that holds the client folders, on the office file
   server (`\\server\Clients`) or a local disk for a one-PC office.
2. On the first workstation: Vibe Files › Cabinet › **Create new**, pick the folder, name the
   cabinet. If the folder already holds client folders (an earlier Filer Desktop tree, a
   FileCenter cabinet, a plain firm tree), they are indexed in place; nothing moves.
3. **Clients**: Browse › Import clients CSV… from the tax software's client export. Tick "keep the
   last 4 digits only" on the SSN/EIN column (needed for form recognition; the full number is
   never kept).
4. **Existing folders**: Admin › Folder import binds the old tree's folders to the clients (by
   the ID in the folder name or the same name), gives files their doc type and tax year from
   their names and folders, and lists what is left for a person (export the list as CSV). Undo
   the last import puts everything back.
5. **Intake**: the Intake tab is Filer Desktop. Set its inbox folders. Only **one workstation
   runs Intake at a time** (v1 has no catalog lock; PA-16).
6. **Scanner**: Intake › Scanner on each scanning PC (TWAIN for the fi-7160, or WIA).
7. **OCR**: needs nothing (the bundled `tesseract\`); an imported cabinet's backlog is read in
   the background, or at once with `vibe-files ocr backlog <cabinet>`.
8. **Sync** (offsite copy in B2): see §4.

Other workstations: install (§1), Cabinet › **Open existing**, the same folder. Each
workstation keeps its own catalog of the shared cabinet (Q-26).

## 3. Sharing the cabinet folder on the LAN

- Share the cabinet folder on the server; give the firm's staff group Modify, nobody else
  anything. Windows Explorer opens every file in it without Vibe Files (principle 1).
- Keep `<data>\` on each workstation's local disk, never on the share: SQLite over SMB is not
  safe.
- One Intake writer at a time (above). Other changes (metadata, versions, trash, renames) can be
  made from any workstation; each workstation sees another's changes after its next rebuild or
  integrity sweep, or at once when both sync through B2.
- The weekly integrity sweep (Admin › Integrity sweep) re-hashes the cabinet and catches files
  edited, moved or added outside Vibe Files.

## 4. B2 sync and key escrow

Set up once per cabinet, then joined on each workstation that syncs:

1. In Backblaze B2: create a **private** bucket; optionally turn on **Object Lock** (Bucket
   Settings; 30 days suggested, Q-10): no one, ransomware included, can then change or delete
   an object for that long. Create an **application key** limited to that bucket.
2. Vibe Files › Sync › Set up sync: the bucket's S3 endpoint (`https://s3.<region>.backblazeb2.com`),
   region, bucket name, key ID, application key, and a **sync passphrase** (12+ characters).
3. On another workstation: the same form with the same bucket and the **same passphrase** joins.
4. Unlock: with "Remember on this PC for my Windows login" ticked (the default), the key is
   sealed with Windows DPAPI for that login on that PC and sync unlocks itself every time Vibe
   Files starts; untick it to type the passphrase each session. Sync › Forget on this PC undoes
   it (do that before a PC leaves the firm). While unlocked, Vibe Files pushes every 15 minutes
   (setting; optionally only within an hour window) and on Push now.

Everything is encrypted on the workstation before upload (XChaCha20-Poly1305 with the firm sync
key); object names reveal no client, file or hash (docs/BUCKET_LAYOUT.md).

**Key escrow.** The firm sync key exists only wrapped by the sync passphrase: in the bucket's
`cabinet.json` and in `<cabinet>\_Vibe\keys\sync.json`. **The passphrase is the escrow:**

- Write it down once and keep it with the firm's recovery papers (the safe, the attorney's file),
  and in the firm's password manager. Two partners should know where.
- Without it, the bucket cannot be read by anyone, Kisaes included. There is no reset.
- Changing staff does not require a new passphrase unless one of them knew it and left; then set
  up a new bucket with a new passphrase and push again.

The B2 application key is stored in `config.toml` sealed with the firm key, never in the clear.
A remembered key (`<data>\sync-key-<cabinet id>.dpapi`) opens only for the Windows login that
saved it on that PC; anyone who can sign in as that login can sync, so keep Windows logins
personal and locked.

## 5. Disaster recovery from the bucket

When the server or the cabinet folder is lost:

1. On any PC: install Vibe Files (§1).
2. On a bare PC, from a command prompt (the application key and the passphrase are asked for,
   or taken from `VF_S3_APP_KEY` / `VF_SYNC_PASSPHRASE`):

   ```
   vibe-files restore D:\Clients --endpoint https://s3.us-west-004.backblazeb2.com ^
     --region us-west-004 --bucket firm-vibe --key-id <key id>
   ```

   lists the cabinet ids in the bucket; run it again with `--cabinet-id <id>`. `D:\Clients`
   must be new or empty. From a workstation that still has sync unlocked: Sync › Restore cabinet
   to a folder…. Either way (tested: `vf-fixtures/tests/sync_restore.rs`) every file comes back
   byte-identical, client folders keep their sentinels (Filer's root scan shows them bound), with
   versions, doc types, years and the audit trail's monthly files.
3. Open the restored folder as the cabinet (Cabinet › Open existing).
4. Check: Admin › Audit trail › Verify now; `vibe-files check <cabinet>`.

One client only: Sync › Pending by client › Restore to a folder… writes that client's current
documents as plain files.

## 6. Health and the command line

At every open Vibe Files puts back any change a crash interrupted (the transaction journal), then
checks the catalog's integrity, the audit chain, that the cabinet answers and takes writes, and
the sync state; anything wrong shows on Browse. The same from a command prompt:

| Command | What |
|---|---|
| `vibe-files check <cabinet>` | the self-checks; exit 0 = OK, 5 = something failed |
| `vibe-files audit verify` | the audit hash chain; exit 0 / 2 |
| `vibe-files rebuild <cabinet>` | catalog rows from the folder tree (safe to repeat) |
| `vibe-files ocr backlog <cabinet>` | read every PDF not yet searchable |
| `vibe-files sync <cabinet> push\|pull\|status` | sync (passphrase from `VF_SYNC_PASSPHRASE`, else typed: not shown) |
| `vibe-files restore <folder> --endpoint … --region … --bucket … --key-id … [--cabinet-id …]` | disaster recovery from B2 (§5) |
| `vibe-files export-client <cabinet> <ID> <out.zip>` | a client's files + audit extract (CSV) + manifest (SHA-256 per file) |
| `vibe-files soak --hours 8` | the leak test on a synthetic cabinet (P13) |

Every command takes `--data <folder>` for a data folder other than the default. Commands that
change things take the data folder's lock: with the app open on the same data folder they stop
with exit code 7 (close the app, or use `--data`). `check` only reads.

`vibe-files.exe` is a windowed program: cmd and PowerShell do not wait for it, so for the exit
code run it as `start /wait vibe-files.exe check <cabinet>` (cmd) or
`Start-Process vibe-files.exe -ArgumentList 'check','<cabinet>' -Wait -PassThru` (PowerShell),
or from Task Scheduler.

Logs: `<data>\logs\vibe-files.<date>.log`, 30 days kept; actions, counts and error kinds only,
no names, paths or document contents.

## 7. Release (Kisaes)

1. Bump `version` in `Cargo.toml`; update STATE.md.
2. Commit everything, then `pwsh scripts/release.ps1`: static-CRT build, the portable folder
   with pdfium, TWAINDSM and Tesseract (only the DLLs it loads), `vibe-files-<ver>-win64.zip`
   (no top folder: the exe is at its root), `SHA256SUMS.txt`, and the winget manifests under
   `target/dist/winget/`. It refuses a tree with uncommitted changes (`-AllowDirty` makes a
   trial build, never to publish) and prints the commit it was built from.
3. Smoke test the zip on a clean PC (open a cabinet, view a PDF, OCR one scan, push).
4. Tag `v<ver>`, create the GitHub release, attach the zip and `SHA256SUMS.txt`; put the SHA-256
   lines in the release notes.
5. winget: open a pull request to `microsoft/winget-pkgs` with the generated
   `manifests/k/Kisaes/VibeFiles/<ver>/` files (`wingetcreate submit` or by hand). The first
   submission is reviewed by Microsoft; later ones are routine.
