# Adding files and new clients

How documents get into the cabinet, and how to add a client with its folders.

## Ways to add documents

- **Copy them in with Explorer.** Drop files straight into a client's folder. Vibe Files finds them on its own (see below).
- **Intake.** Put files in an inbox folder, scan to the Scans folder, or print to the print-capture folder; Intake matches them to clients and files them after you check the preview. See [Intake](help:intake) and [Scanning](help:scanning).
- **Scan from Browse.** With a client's folder shown, **Scan** files the scan straight into it. See [Scanning into a folder and printing from Browse](help:browse-scan-print).

## Files copied in with Explorer

Browse looks at the client folders on disk every 30 seconds. Any new file in a client folder is added to the catalog as a document, and a note tells you, for example:

**Added 2 new files found in client folders: 2025 W-2.pdf, 1099-INT.pdf. Set the doc type in the inspector.**

The new documents have no doc type yet, so they show the **Needs doc type** pill and appear under **Needs doc type** in **VIEWS**. Select them and set the doc type and tax year in the details pane. See [Document details](help:documents).

Left out on purpose:

- Office and Explorer working files, such as `~$letter.docx`, `desktop.ini` and `Thumbs.db`.
- A file with exactly the same content as a document already in the catalog (a copy, or a file moved in Explorer). The **Integrity sweep** in Admin deals with those. See [Maintenance](help:maintenance).
- Files outside any client folder.

## New client

1. On Browse, click **+ New client** above the grid.
2. Fill in **Client ID**, for example `1004`. If that ID is already in your client list, the name is filled in for you and shows **(from the client list)**.
3. Otherwise type the **Client name**, for example `Baker Farms`.
4. Choose an **Entity type**, or leave it **Not set**.
5. Under **Creates**, check the folders that will be made, such as `Baker Farms (1004)/Income Tax`, and the client's `_Vibe/client.json`.
6. Click **Create client (one transaction)**.

The new client is picked in the tree and a note says **Created** with its folder name. **Undo last** takes the new client back (only folders that are still empty are removed). See [Undoing changes](help:undo).

> If a client from the client list has no folder yet, pick it in the tree and click **Create folder…** in the right pane, or click **+ New client** with it picked: its ID is filled in.

To add many clients at once from a spreadsheet, use **Import clients CSV…**. See [Clients](help:clients).

## The client folder template

The folders a new client gets come from the cabinet's folder template, set in **Admin** › **Folder templates**. Year folders (`{Year}`) are not made up front; they are made when something is filed for that year. Changing the template affects new clients only; existing client folders are not changed. See [Folder templates](help:folder-templates).

## An empty cabinet

A cabinet with no documents shows **Create your first client or run the import wizard.** To bring in an existing folder tree, see [Admin](help:admin) (**Folder import**).
