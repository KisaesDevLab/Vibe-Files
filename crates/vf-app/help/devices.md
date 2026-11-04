# Devices

How Vibe Files tells the firm's PCs apart, the Devices page that lists them, and taking a PC that is gone off the list.

## Each PC has its own id

The first time Vibe Files runs on a PC, it gives that PC a device id: a long random code kept in `config.toml` in the PC's data folder (see [where your data lives](help:data-and-backups)). Every change that PC makes carries its device id, which is how the [audit trail](help:audit) and [sync](help:sync) know which PC did what. The same id is kept when Vibe Files is upgraded, as long as the data folder stays.

Each PC also keeps its own catalog of the shared cabinet. See [several PCs](help:several-pcs).

## The Devices page

Open **Admin** › **Devices**. It lists every workstation that has opened this cabinet, recorded when each opens it:

- **COMPUTER**: the PC's Windows name. Your own shows "(this PC)".
- **VERSION**: the Vibe Files version it ran last.
- **FIRST SEEN**: when it first opened the cabinet.
- **LAST OPENED**: when it last did.

The list is kept in the cabinet (`_Vibe\devices`), so every PC sees the same list. Use it to check which PCs still use the cabinet and whether each is on the current version.

## Removing a PC

When a PC is replaced or leaves the firm, take it off the list:

1. Open **Admin** › **Devices**.
2. Press **Remove** on its line ("Take this PC off the list").

You see "Removed from the list." and the removal is recorded in the audit trail. You cannot remove the PC you are using.

> Removing only takes the PC off the list. If it opens the cabinet again, it comes back. To stop it reaching the cabinet, take away its access to the shared folder in Windows.

## Before a PC leaves the firm

- If sync remembered its key for a Windows login, open **Sync** on that PC and press **Forget on this PC** (admin password needed). See [the sync key](help:sync-key).
- Its data folder holds a full catalog, client names and document text included. Delete the data folder, or wipe the disk, before the PC goes.
- Then remove it here.
