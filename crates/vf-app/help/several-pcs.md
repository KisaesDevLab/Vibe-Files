# Several people filing at once

Anyone can file from Intake on any PC at the same time. This article explains what each PC keeps to itself, what the PCs share, and the few things to watch for.

## Each PC has its own Intake folder

When you open the **Intake** tab, Vibe Files gives this PC its own working folder in the cabinet, `_Vibe\intake\<PC>`. In it are this PC's:

- last batch, and so its own **Undo last batch** on the **History** screen
- record of each step a batch took
- working PDFs (split pieces, combined files, files kept for undo)

So when Ann on one PC and Bob on another both file a batch, each can undo only their own last batch. Neither undoes the other's. Every PC's batches are recorded in every PC's catalog, so Browse on any PC shows what anyone filed.

## What the PCs share

These Intake files are shared by every PC:

- settings (inboxes, client CSV, ID pattern)
- rules
- client list
- relationships
- folder template
- the folder list of the cabinet

Intake reads a copy of each when it loads. Every few seconds, a change saved on this PC is copied to the cabinet, and a change saved on another PC is copied here. Light or dark theme and text size are each PC's own and never travel.

## Reload Intake

When the client list, settings, rules or template change somewhere else (another PC, or Admin on this one), a banner appears above Intake, for example: "The client list or Intake's settings changed (another PC, or Admin). Reload Intake to use them…". Press **Reload Intake** to load the new ones.

- The button is greyed out while this PC is filing a batch. Wait for it to finish.
- Reloading reads the inbox again. Changes you made on rows in **Preview** and did not commit (a client you chose, a rename, a skip) are lost, so commit first if you can.

A new client added in Browse also shows this banner: Intake offers the new client after the reload.

## Things to watch for

- Two people change the same setting or rule at the same moment: the last save wins; the other person's change is lost. Agree who edits the rules, or check them after a reload.
- Two people file the same inbox file: if two PCs watch the same inbox, both see the same files. The first to commit files it; the second gets a "file not found" error for that row. Nothing is lost or filed twice. Separate inbox folders for each person avoid this.

> See [Filing documents with Intake](help:intake) for the queue and commit, and [rules and Setup](help:intake-rules) for the shared settings.

## Your own inbox (several people on one server)

When several people work on one Remote Desktop server, each can have inbox folders of their own. On the Intake tab, **My inboxes** › **Add folder…** adds one (any folder you can write to, outside the cabinet; not a shared inbox, the Scans folder or the print capture folder); **×** stops watching it. Only your Windows login watches it; nobody else sees its files, and the cabinet's shared settings do not change.

Intake's Preview then has two tabs, **Shared inbox (n)** and **My inbox (n)**. Each tab is its own batch: **Commit** files only that tab's rows, the same way in both. Scans and print captures still arrive in the shared inbox. A file moved to the trash from your inbox goes back to it on **Restore**.

## One catalog on a server

On a Remote Desktop server where several people use one cabinet, each login would otherwise build its own catalog by reading the whole cabinet. Instead, once: **Admin** › **Devices** › **Share this catalog with everyone on this PC**. The catalog is copied to the PC's own disk (`ProgramData\Vibe Files\catalogs`) and every login of the PC uses it from their next open. Each person keeps their own settings, undo and cache, and sees the others' changes within a few seconds.

## Several Remote Desktop servers

When Vibe Files runs from a network folder (for example `\server\share\APPS\Vibe Files`) and people log into several servers, each server works like this:

- **Your settings stay on the server you are on**, in `%LOCALAPPDATA%\Vibe Files\data`. Nothing is kept in the network folder. The first time, the settings from the old data folder beside the program are copied over for its owner.
- **Each server keeps one catalog for everyone who logs into it.** The first server indexes the cabinet once and leaves a copy in the cabinet's `_Vibe\catalog-seed` folder (refreshed weekly). The other servers start from that copy and then only check file sizes and dates, so they don't index everything again.
- **Changes reach the other servers.** A move, a rename, or a doc type or year set on one server shows on another when that client is opened there, or within 15 minutes.
- **Text recognition and B2 sync run on one server.** On the others, scans become searchable once that server has read them and written their text into the files. **Admin** › **Devices** › **Servers** shows which server it is, and **Run them on this server** moves the job (from the next start of Vibe Files).

