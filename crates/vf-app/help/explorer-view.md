# Explorer view

How to flip Browse's list between the documents of what you selected and the folder as it is on disk, like Windows Explorer.

## The switch

Above the list, beside **Find documents…**, are **Documents** and **Explorer**.

- **Documents** is the list Vibe Files has always shown: the documents of the client, folder or view selected on the left, including those in the folders below it, with their doc type, year and status.
- **Explorer** shows one folder as it is on disk: its folders first, then every file in it, with **Name**, **Date modified**, **Type** and **Size**. Click a column's name to sort by it.

Your choice is kept for your Windows login on this PC. On a server that is reset each night it starts as **Documents** again the next day.

**Explorer** is available when what is selected on the left is a folder on disk: the cabinet, a client or a folder, with the tree set to **Folders**. For a view, a saved search, what **Find documents…** found, or the tree set to **By year**, the list stays as **Documents**.

## Moving around

- Double-click a folder to go into it. **Up** goes to the folder it is in, and stops at the cabinet's own folder.
- The tree on the left follows when the folder is a client's or inside one. Clicking in the tree moves the list too.
- The line above the list says where you are; point at it for the full path.
- **Filter this folder** narrows the rows by name.

## Files and what you can do with them

A file the catalog has is the document. Click it and the toolbar and the details pane on the right work on it exactly as in **Documents**: open, rename, move, share, print, trash. Double-click opens it.

A file shown fainter is one the catalog does not have: a file put there outside Vibe Files that has not been read in yet, or one of Vibe Files' own working files under `_Vibe`. You can open it in its Windows program (double-click), show it in Windows Explorer or copy its path from the right-click menu. Nothing else is offered, so nothing is moved or renamed without being recorded. While the cabinet is still being listed after a first open, most files look like this until their folder has been listed.

Folders can be opened and shown in Windows Explorer. To rename or move a folder, use the tree on the left.

## What it reads

Each folder is read from the share when you go into it, and again after **Refresh** or after a change to the catalog. A spinner shows while a slow share answers.

> Explorer view shows everything in the folder, including the `_Vibe` folders Vibe Files keeps its own records in. Leave those files as they are.
