# Folder templates

How to change the folder tree every new client gets: the tree, the {Year} folders and other tokens, checking it, and saving it.

## What the template is

The template is the set of folders a new client gets. It is kept in `_Vibe\template.json` in the cabinet, in Filer Desktop's format, so Intake files into the same tree. The standard tree is:

- `Income Tax/{Year}`, `Bookkeeping/{Year}`, `Payroll/{Year}`, `Financials/{Year}`
- `Correspondence/{Year}`, `Workpapers & Support/{Year}`, `Sales Tax/{Year}`
- `Permanent/Setup` and `Permanent/Stock Basis`
- `Other/Practice/{Year}`, `Other/{Year}`, `Not Yet Filed`

> Changing the template affects new clients only. Existing client folders are not changed, renamed or moved.

## Editing the tree

Open **Admin** › **Folder templates**. The tree is on the left; click a folder to work on it on the right.

- The top line is the **Client folder** itself. Its name uses tokens: `{Client}` (the client's name), `{ID}` (the client ID) and the client list's columns you chose to keep. The standard name is `{Client} ({ID})`, which gives folders like `Baker Farms (1004)`.
- **Name** and **Rename**: type a new name for the selected folder, then press **Rename**.
- **Add sub-folder**: adds `New folder` inside the selected one; rename it.
- **Add {Year} folder**: adds a folder per tax year inside the selected one. Year folders are made when something is first filed for that year, not in advance.
- **Move up** and **Move down**: change the order among its neighbours.
- **Remove**: takes the folder out of the template only. Folders already on disk stay.

## Checking and saving

Vibe Files checks the tree as you edit. Any problem (an unknown token, two sub-folders with the same name, `{Year}` in the client folder's own name) shows in red under the tree, and **Save template** stays greyed until it is fixed.

- **Save template** keeps the changes: "Template saved: new clients get these folders." The change is recorded in the [audit trail](help:audit).
- **Discard changes** goes back to the last saved tree.
- **Restore Filer's default** puts back the standard tree; press **Save template** to keep it.

Other PCs pick up a saved template too: their Intake offers **Reload Intake**.

## Related

- [Doc types](help:doc-types) name a default folder from this template, for example `Income Tax/{Year}`.
- [Clients and relationships](help:clients) covers creating a client, which makes its folder from the template.
