# Backing up and syncing to Backblaze B2

How the Sync tab keeps an encrypted copy of the cabinet in your firm's Backblaze B2 bucket, who can set it up, who can run it, and what never leaves the office.

## What sync does

Sync is two-way. A push sends what changed on this PC to the bucket; a pull brings down what other PCs sent. A push always pulls first, so it never hides another PC's work.

Everything is encrypted on this PC before it leaves, with the firm's sync key. The names in the bucket reveal no client, file or hash. What goes up: every document and its earlier versions, each client folder with its `_Vibe\client.json`, the doc types, the folder template, the client list, retention classes, legal holds, Intake's rules and settings, and the audit trail's monthly files. Taxpayer-ID columns are stripped from the client list before it is sent.

## Setting it up

Setting up needs the admin password (see [the admin password](help:admin-password)). Without it the Sync tab says **Sync is not set up on this PC.**

1. In B2, create a private bucket and an application key for it.
2. In Vibe Files, open **Sync** and fill in **S3 endpoint**, **Region**, **Bucket**, **Key ID**, **Application key**, **Sync passphrase** (at least 12 characters) and **Passphrase again**.
3. Press **Connect**. The first push sends the whole cabinet.

On a second PC, enter the same bucket and the firm's existing passphrase to join. If the firm keeps the key in the cabinet, other PCs sync without any set-up (see [the sync key](help:sync-key)).

## Running it

Once sync is unlocked on a PC, anyone at that PC can press **Push now**, **Pull now** or **Verify now** (downloads a sample of documents and checks each against its SHA-256). The **Pending by client** list shows what is waiting; each client has its own **Push**. In Browse, the **Sync to B2** menu offers **Push this client now**, **Pull latest**, **Verify (sampled)** and **Show in Sync tab**.

The status grid shows **Status**, **Bucket**, **Last push**, **Last pull**, **Manifest rev** and **Automatic**.

## Automatic push and pause

Under **Settings** (admin password needed), **Push automatically every (minutes, 0 = by hand)** sets the timer. It is 15 minutes on a PC where sync was set up, and by hand on a PC that joined through the shared key until someone sets it there. **Only push automatically between** limits automatic pushes to a window of hours. **B2 Object Lock is on for this bucket** records that the bucket protects its objects for a number of days.

**Pause** (admin password needed) stops the automatic pushes; **Resume** starts them again. **Push now** still works while paused.

## Pushing while Vibe Files is closed

Automatic pushes happen only while Vibe Files is open. On several servers, or a cabinet in OneDrive or Dropbox, only the main computer pushes (**Admin** › **Devices**). The cabinet is not lost while that computer's Vibe Files is closed, but B2 falls behind. Every computer's startup check says so when the cabinet was last sent over 24 hours ago.

To push with nobody signed in, use Windows Task Scheduler on the main computer:

1. Use a Windows login that does not keep Vibe Files open (a login of its own for the task is best). Sign in there as it, open Vibe Files, unlock sync with **Remember on this PC for my Windows login** ticked, and close it. A key kept in the cabinet also works.
2. Create a task that runs as that login, **whether the user is logged on or not**, with its password stored, repeating every hour.
3. Its action: `vibe-files.exe sync "<cabinet folder>" push`.

The task first reads what the other computers changed, then pushes. It exits with 0 when it sent the changes (or another session was already syncing), 7 when Vibe Files is open for that login (nothing sent), and 8 on a computer that is not the main one.

## A large cabinet's first upload

The first push of a large cabinet (hundreds of gigabytes) can take days. It goes up 100 clients at a time, and each group is saved in the bucket as it finishes: stopping, closing Vibe Files or losing the connection loses at most the group being sent, and the next push carries on from there. Eight files are sent at once; files over 32 MB are sent one at a time.

**Upload at most (megabits a second, 0 = no limit)** under **Settings** keeps the office line usable meanwhile. It applies to this PC only. **Only push automatically between** can also keep the first upload to the evening and night.

## What is never synced

- `_Vibe\keys\`: the sync key files.
- Filer's working folders (`_Vibe\tmp`, `_Vibe\overwritten`, `_Vibe\undo_zips`, `_Vibe\split_originals`).
- `_Vibe\locks\`.
- This PC's data folder: `catalog.db`, `config.toml`, caches and logs (see [where your data lives](help:data-and-backups)).

> Without the sync passphrase nobody can read the bucket, Kisaes included. Keep it with the firm's recovery papers.

See also [conflicts and restoring](help:sync-conflicts).
