# Troubleshooting

Messages you may meet in Vibe Files, what they mean and what to do. The words in quotes are what the app shows.

## Intake

### "The client list or Intake's settings changed… Reload Intake to use them"

Someone changed Intake's rules, settings or folder template, on this PC or another one (new clients and relationships come in on their own). Press **Reload Intake**. Files you had not yet confirmed in Intake's review are read again from the inbox; nothing filed is lost. If the button is greyed ("Intake is filing a batch: wait for it to finish."), wait a moment. A similar banner appears after **Print capture & parsers** changes. See [several PCs](help:several-pcs).

### "Intake batch not recorded yet (trying again in N s)"

The files were filed, but recording the batch in the catalog has to wait, often because a file is still being written. Vibe Files tries again on its own.

### "file not found" for a file in a shared inbox

Another PC filed it first. Nothing is lost; look for it in Browse.

## Scanning

### "Scanned, but not filed"

Scanning from Browse worked, but the scan could not be filed into the folder shown. The rest of the message says where it is: either "The scan went to the Scans folder for Intake", so file it from [Intake](help:intake), or "The scan is kept at" a path, so open that folder and file the PDF by hand.

## Sync

### "Unlock sync first (Sync tab)."

Sync is set up but not unlocked on this PC. Open **Sync**, type the **Sync passphrase** and press **Unlock**. See [the sync key](help:sync-key).

### "the sync passphrase is wrong"

Check Caps Lock and try again. There is no reset; ask the person who keeps the firm's recovery papers.

### "Needs the admin password (Admin tab)."

Sync set-up and settings need the admin password. Unlock **Admin** first, then come back to **Sync**.

## Admin password

### "Too many wrong passwords: try again in N s."

After three wrong tries the password is not checked for a while. Wait, then type it carefully. Forgotten: see [the admin password](help:admin-password).

## A file in use

A message containing "being used by another process" means the file is open in another program (Acrobat, Word, Excel, Explorer's preview pane). Close it there and try again. Intake reports such a file on its own row and files the rest of the batch.

## Printing

### "The print was NOT recorded in the audit trail"

The pages went to the printer, but the audit row could not be written, often because the cabinet folder was briefly unreachable. Note what was printed and tell whoever looks after Vibe Files. See [printing](help:printing).

## Starting Vibe Files

### "Vibe Files is still running under your login"

Starting Vibe Files a second time brings the window already open in your session to the front. When the other copy is in another of your sessions on the server (with Citrix or Remote Desktop, a session that was disconnected instead of signed out keeps its programs running) or has stopped responding, Vibe Files asks **Close that copy and open Vibe Files here?** Yes closes it and opens Vibe Files; anything not saved in that copy is lost. To avoid it, sign out of the server (Start › your name › Sign out) instead of just closing the remote window.

### "Vibe Files is already open under your login on this computer"

Vibe Files is already running for your Windows login on this computer, in another window. Switch to that window (check the taskbar), or close it first.

Someone else having Vibe Files open never stops you: when another person, or you on another computer, is using the data folder beside the program, Vibe Files uses your own data folder (`%LOCALAPPDATA%\Vibe Files\data`) instead. On a server where several people work, share the cabinet's catalog once (Admin › Devices › **This PC's catalog** › Share) so that everyone uses the same catalog instead of each indexing the cabinet. See [several PCs](help:several-pcs).

### "Vibe Files is already open with this data folder"

You started Vibe Files with `--data` and that folder is in use by another running copy. Close it there, or use another folder.

### "The catalog in … belongs to a different cabinet"

This PC's catalog belongs to another cabinet. Open the cabinet you normally use, or ask whoever looks after Vibe Files.

### "Startup check:" on Browse

A check failed when the cabinet opened. See [rebuild, sweep and checks](help:maintenance).

### "The audit trail does not verify at row N"

See [the audit trail](help:audit) and tell whoever looks after Vibe Files.
