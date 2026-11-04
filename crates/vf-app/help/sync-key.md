# The sync key

What the sync passphrase protects, how a PC unlocks sync, remembering the key for your Windows login, letting everyone who opens the cabinet sync, and what happens if the passphrase is lost.

## Key and passphrase

When sync is first set up, Vibe Files makes one firm sync key. It encrypts everything sent to the bucket. The key is never stored in the clear: it is kept wrapped by the **Sync passphrase**, in the bucket and in `_Vibe\keys\sync.json` in the cabinet. That folder is never uploaded.

The passphrase is the only way to the key. Write it down once and keep it with the firm's recovery papers and in the firm's password manager.

## Unlocking sync on a PC

On a PC where sync is set up but not unlocked, the Sync tab says **Sync is set up for this cabinet. Unlock it for this session:**. Type the **Sync passphrase** and press **Unlock**. A wrong one gives "the sync passphrase is wrong".

**Lock** closes sync again until it is unlocked (wait for a running sync to finish first).

## Remember on this PC for my Windows login

With **Remember on this PC for my Windows login** ticked (it starts ticked), the key is sealed with Windows for your login on this computer and kept in this PC's data folder. Sync then unlocks itself each time you start Vibe Files. Another login or another computer cannot open it. Untick it to type the passphrase every session.

**Forget on this PC** (admin password needed) removes the remembered key: sync asks for the passphrase again. Do this before a PC leaves the firm.

> Anyone who can sign in as your Windows login on that PC can sync. Keep Windows logins personal and locked.

## Let everyone who opens this cabinet sync

An admin can tick **Let everyone who opens this cabinet sync without the passphrase** when setting up or unlocking. The sync key and the bucket settings are then kept in the cabinet's `_Vibe\keys` folder. Anyone who can open the cabinet on the network can sync with no set-up and no passphrase, while the copy in Backblaze stays encrypted.

On a PC that joins this way, automatic push starts as by hand; an admin sets the timer under **Settings** on that PC.

**Stop sharing the key** (admin password needed) removes the key kept in the cabinet. Everyone then needs the passphrase, or their own remembered key, again.

## If the passphrase is lost

There is no reset. Without the passphrase the bucket cannot be read by anyone, Kisaes included. PCs that still have sync unlocked (remembered or shared key) keep working, but a new PC cannot join with the passphrase and a bare PC cannot restore the cabinet from the bucket.

If the passphrase is lost, or someone who knew it leaves the firm, ask whoever looks after Vibe Files to set up a new bucket with a new passphrase, and push the whole cabinet again.

See also [syncing to B2](help:sync) and [the admin password](help:admin-password).
