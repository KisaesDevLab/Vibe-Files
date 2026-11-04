# The admin password

One password for the whole cabinet: what it protects, how to set, change, remove and lock it, what happens after wrong tries, and what to do when it is forgotten.

## What it protects

- The whole Admin tab, on every PC that opens the cabinet.
- The Sync tab's set-up and **Settings**, plus **Pause**, **Restore cabinet to a folder…**, **Forget on this PC**, **Stop sharing the key** and the box that shares the key. Anyone can still push, pull and verify once sync is unlocked.
- Emptying the trash by hand, destroying approved documents, and releasing legal holds: these ask for it again.

Until a password is set, anyone can change Admin and Sync settings, and the trash cannot be emptied by hand.

## Setting it

1. Open **Admin** › **Admin password**.
2. Type it in **New password** and **New password again** (at least 6 characters).
3. Press **Set admin password**. You see "The admin password is set."

Only a scrambled hash is stored, in `_Vibe\trash_pin.json` in the cabinet folder. The password itself is never written down or logged.

## Changing or removing it

Type the **Current password**, then:

- a new one twice and press **Change admin password**, or
- press **Remove admin password**: Admin and Sync settings are open to everyone again.

If the two new passwords differ you see "The two passwords differ."; a wrong current one gives "the current admin password is not right".

## Unlocking and locking

When Admin is locked, type the password under **Admin is locked** and press **Unlock** (or Enter). It stays unlocked on this PC until Vibe Files closes. Press **Lock Admin** at the top of the Admin list to lock it again straight away, for example before you leave your desk.

Every unlock, and every refused try, is recorded in the [audit trail](help:audit). The password is never in it.

## Wrong tries

A wrong password says "That admin password is not right." After three wrong tries in a row the password is not checked for a while, and you see "Too many wrong passwords: try again in N s." The first wait is 30 seconds; it doubles with each further wrong try, up to 10 minutes. The right password resets the count.

## Forgotten password

1. On the server, open the cabinet folder and delete `_Vibe\trash_pin.json`.
2. In Vibe Files, open **Admin** › **Admin password** and set a new one.

The audit trail records the new password as a reset (the file had been removed), so going around the password always shows. With B2 sync, the removal goes to the other places at the next sync; the old password does not come back.

> Anyone who can change files in the cabinet folder can do this. The password keeps settings from being changed by mistake; it is not a lock against someone with access to the server.

If `_Vibe\trash_pin.json` is there but cannot be read (a network hiccup, a damaged file), Vibe Files treats the password as set and keeps Admin locked.
