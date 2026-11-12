# Opening or creating a cabinet

The **Cabinet** screen: opening the firm's existing cabinet, creating a new one, and what happens on later starts.

## The Cabinet screen

When no cabinet is open, Vibe Files shows the **Cabinet** screen. At the top you choose between two modes:

- **Open existing** — the cabinet already exists (another PC in the office set it up, or you did before).
- **Create new** — make a folder into a cabinet for the first time.

## Open an existing cabinet

1. Click **Open existing**.
2. In **Cabinet folder**, type the folder path, for example `C:\Clients` or `\\server\share\Clients`, or click **Browse…** to pick it.
3. Click **Open cabinet**.

While it opens you see **Opening the cabinet…**. The first time a PC opens a cabinet, Vibe Files indexes the whole tree: it reads every file once to fingerprint it, several at a time. On a large cabinet this takes a while, and the screen shows how far it is (**Indexing: N of M files read**). Later opens are quick.

If the folder is not a cabinet, the screen shows a message saying it is not a Vibe Files cabinet. Check the path, or use **Create new**.

## Create a new cabinet

1. Click **Create new**.
2. In **Cabinet folder**, give the folder that holds (or will hold) the client folders.
3. In **Cabinet name**, type the name shown in the header, for example `Baker Firm Clients`.
4. Click **Create cabinet**.

You see **Creating the cabinet and indexing its folders…** while it works.

> The folder may already hold client folders. They are indexed where they are, not moved.

If something is missing, the screen tells you: **Choose the cabinet folder.**, **Give the cabinet a name.**, or **This folder is already a cabinet. Use Open instead.**

## Later starts

Vibe Files remembers the last cabinet each Windows login opened and opens it on its own when you start the program. You do not see the **Cabinet** screen at all.

While it is opening, the **Open a different cabinet** button is shown under **Opening the cabinet…**. Click it to go back to the **Cabinet** screen and pick another folder.

If the last cabinet cannot be opened (the share is down, or the folder was moved), the **Cabinet** screen shows with the reason, and the folder filled in so you can try again or change it.

> Each PC keeps one catalog for one cabinet. If the screen says the catalog belongs to a different cabinet, that PC's data folder is already used for another cabinet; ask whoever set up the PC. See [Data and backups](help:data-and-backups).

## The firm's cabinet for every login

So that nobody has to pick the cabinet the first time, an admin can name it once: put a file called `firm.toml` beside `vibe-files.exe` (in the folder on the share, or in each PC's copy) holding one line, for example `cabinet = '\\server\share\Documents'`.

Keep the single quotes. A login that has not opened a cabinet yet then opens that one straight away; after that, its own last cabinet. **Open a different cabinet** still works. A winget install replaces its program folder on each update, so there put the file back after updating.

## Next

- [Finding your way around](help:screen-tour)
- [Browsing the cabinet](help:browse)
- [Using several PCs](help:several-pcs)
