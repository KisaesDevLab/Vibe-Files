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

## What is never synced

- `_Vibe\keys\`: the sync key files.
- Filer's working folders (`_Vibe\tmp`, `_Vibe\overwritten`, `_Vibe\undo_zips`, `_Vibe\split_originals`).
- `_Vibe\locks\`.
- This PC's data folder: `catalog.db`, `config.toml`, caches and logs (see [where your data lives](help:data-and-backups)).

> Without the sync passphrase nobody can read the bucket, Kisaes included. Keep it with the firm's recovery papers.

See also [conflicts and restoring](help:sync-conflicts).
