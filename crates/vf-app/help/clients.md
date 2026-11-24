# Clients and relationships

Where the client list comes from, adding a client and its folder, how a folder is tied to its client, relationships that send K-1 copies to owners, and adopting an existing folder tree.

## The client list

The client list comes from your tax software's client export. In Browse, press **Import clients CSV…**, choose the file with **Browse…** and press **Read**. Then:

- pick the **Client ID column** and the **Name column**;
- under **Other columns to keep (folder-name tokens)**, tick any extra columns you want to use in folder names;
- for a taxpayer-ID column, tick its **keep the last 4 digits only** box. Full SSNs and EINs are never copied into the client list; the last four digits help match W-2s, 1099s and K-1s to clients.

- under **Portal contacts**, the email and cell columns ([portal invitations](help:portal-invite-list)).

**Changes in this CSV** shows new, renamed and changed clients, and those this CSV leaves out (they leave the list; their folders stay). Press **Import N rows (one transaction)** to apply it. Re-importing an updated export keeps every client's identity. The file and its columns also become Intake › Setup's client CSV, so both read the same file. The message says how many clients have no folder yet, and names existing folders that match a client but are not tied to it.

## Adding a client

Press **+ New client** in Browse. Type the **Client ID**, the **Client name** (filled in for you when the ID is already in the client list) and, if you like, the **Entity type** and the **Partner** (type it or **Pick…** one other clients have; left empty, the client list's partner is used). **Creates** shows the folder the [folder template](help:folder-templates) gives. Press **Create client (one transaction)**. A client that is not in your client CSV yet is also added to that file (its ID, name and partner; the other columns left blank), so the next import of the CSV keeps it; the notice says so, or why not (the file open in Excel: add it there yourself).

A client from the list that has no folder yet shows **No folder yet (from the client list).** in the client panel, with **Create folder…**.

## Renaming a client

Press **Rename client…** beside the name in the client panel, or right-click the client in the tree. Type the new name: the dialog shows the client folder's new name (`{Client} ({ID})`) and renames the folder with it. Files inside move with the folder. **Undo** puts the folder's old name back; the client keeps its new name until you rename it again. Importing the client list again puts the tax software's name back, so change the name there too.

**Set ID…** beside it changes the client's ID, for a client made from a folder name with a stand-in ID. The client list, relationships, the folder's sentinel and Intake's client CSV take the new ID. It is not on the undo list: set the ID again to change it back. Change the ID in your tax software too, so its next client list matches.

## Merging a client made twice

When a client was made twice, open the one to remove in Browse and press **Merge into…** in its panel. Type the name or ID of the client to keep and pick it. The window shows what happens:

- Every document moves to the same folder under the kept client, with its doc type, tax year and tags.
- A file whose name is already used there is kept too, with "(2)" in its name.
- Legal holds and relationships become the kept client's.
- The merged client's folder, list entry and client CSV row are removed.

The merge is refused while a file in the folder is not in the catalog (press **Refresh** first) or a document is checked out. **Undo last merge** in **Merge into…** puts the latest merge back; documents moved since stay.

## Deleting a client

A client with no documents shows **Delete client…** in its panel and on right-click in the tree. Its folder goes with the empty folders in it, and it leaves the client list, the client CSV and its relationships; the portal is not changed. A file other than Vibe Files' own records in the folder, a document in the trash, or a legal hold stops it. **Undo delete** in Browse's notice puts it back.

## How a folder knows its client

Every client folder holds `_Vibe\client.json`. This small file ties the folder to the client, so the folder can be renamed in Explorer and Vibe Files still knows whose it is. Do not delete or edit it.

## Partner

A client may have a **PARTNER**: who at the firm looks after it. Type it in the client panel and press Enter, or **Pick…** one other clients have; empty means none. A client export's partner column (Preparer, for example) can be chosen as the **Partner column** when you import the client list: clients get that partner unless you set another here. In Browse, **Partner** above the Views shows one partner's clients and documents (or those with **No partner**).

## Relationships

A relationship links an entity to an owner, for example `Baker Farms (1004)` and a partner who owns 40%.

1. Open **New relationship…**.
2. Choose **This client is the entity** or **This client is the owner**.
3. In **Owner client ID** or **Entity client ID**, type part of the other client's name or ID and click the client in the list that appears (its ID is filled in). Then fill in the **Role**, **Share %** and **Note**.
4. Press **Add relationship**.

Intake uses relationships to file copies, for example a K-1 filed to the entity and copied to each owner (see [Intake rules](help:intake-rules)). **Remove** beside a relationship takes it off.

## Importing an existing folder tree

Client folders already in the cabinet (an older firm tree) are adopted with **Admin** › **Folder import**: see [Importing an existing folder tree](help:folder-import).
