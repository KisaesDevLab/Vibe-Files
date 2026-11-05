# Clients and relationships

Where the client list comes from, adding a client and its folder, how a folder is tied to its client, relationships that send K-1 copies to owners, and adopting an existing folder tree.

## The client list

The client list comes from your tax software's client export. In Browse, press **Import clients CSV…**, choose the file with **Browse…** and press **Read**. Then:

- pick the **Client ID column** and the **Name column**;
- under **Other columns to keep (folder-name tokens)**, tick any extra columns you want to use in folder names;
- for a taxpayer-ID column, tick its **keep the last 4 digits only** box. Full SSNs and EINs are never copied into the client list; the last four digits help match W-2s, 1099s and K-1s to clients.

Press **Import N rows (one transaction)**. Re-importing an updated export keeps every client's identity. The message says how many clients have no folder yet, and names any existing folders that match a client but are not yet tied to it.

## Adding a client

Press **+ New client** in Browse. Type the **Client ID**, the **Client name** (filled in for you when the ID is already in the client list) and, if you like, the **Entity type**. Under **Creates** you see the folder the [folder template](help:folder-templates) gives, for example `Smith, John (SMIT1001)` with its sub-folders. Press **Create client (one transaction)**.

A client from the list that has no folder yet shows **No folder yet (from the client list).** in the client panel, with **Create folder…**.

## Renaming a client

Press **Rename client…** beside the name in the client panel, or right-click the client in the tree. Type the new name: the dialog shows the client folder's new name (`{Client} ({ID})`) and renames the folder with it. Files inside move with the folder. **Undo** puts the folder's old name back; the client keeps its new name until you rename it again. Importing the client list again puts the tax software's name back, so change the name there too.

## How a folder knows its client

Every client folder holds `_Vibe\client.json`. This small file ties the folder to the client, so the folder can be renamed in Explorer and Vibe Files still knows whose it is. Do not delete or edit it.

## Partner

A client may have a **PARTNER**: who at the firm looks after it. Type it in the client panel and press Enter, or **Pick…** one other clients have; empty means none. If your client export has a Partner column, tick it under **Other columns to keep** when you import the client list: clients get that partner unless you set another here. In Browse, **Partner** above the client search shows one partner's clients (or those with **No partner**).

## Relationships

Select a client in Browse to see its panel: **ID**, **FOLDER**, **ENTITY TYPE**, **PARTNER** and **RELATIONSHIPS**. A relationship links an entity to an owner, for example `Baker Farms (1004)` and a partner who owns 40%.

1. Open **New relationship…**.
2. Choose **This client is the entity** or **This client is the owner**.
3. Type the other client's ID (**Owner client ID** or **Entity client ID**), the **Role** (shareholder, partner, beneficiary…), the **Share %** and a **Note** if you like.
4. Press **Add relationship**.

Intake uses relationships to file copies, for example a K-1 filed to the entity and copied to each owner (see [Intake rules](help:intake-rules)). **Remove** beside a relationship takes it off.

## Importing an existing folder tree

When the cabinet already holds client folders that are not tied to clients (an older firm tree, often without client IDs in the folder names), do this in order:

1. Import the client list (**The client list**, above). This only saves the list: no folder is renamed, moved or tied to a client yet. Until step 2, Browse shows these clients as **No folder yet**; do not press **Create folder…** for them, or you get a second, empty folder beside the old one.
2. Open **Admin** › **Folder import** and press **Scan the cabinet**. Each top-level folder is matched to a client by its ID or its name, and each file gets a doc type and tax year where its name or folder says so.
3. Check the matches. **Match** shows how well each folder matches its client. **90–100%**: the ID and the name agree, or the same name of two words or more; these are ticked for you. **60–84%**: the same one-word name, or only a similar name. **50% or less**: two folders point at the same client, several clients have the folder's name, the ID is the client's but the name is not, or the client already has a folder. To change a folder's client, pick another in its **Client** column.
4. Tick the folders to import. **Show matches from N% up** lists only folders matched at least that well; **Hide already bound** and **Hide folders with no match** shorten the list. **Tick all shown** and **Untick all shown**, or the **Bind** box at the top of the column, tick or untick every folder listed. Click one folder's box, then Shift-click another to tick or untick all the folders between.
5. To add the client ID to the folder names, tick **Rename bound folders to the firm template's name ("{Client} ({ID})"); files inside are not moved**. For example, `Smith, John` becomes `Smith, John (SMIT1001)`. Left unticked, folders keep their names and are still tied to their clients.
6. Press **Import N folders**. Files are not moved. Ticked folders the filter hides are imported too; the button then says how many, for example **Import 12 folders (3 not shown)**.

**Export the remainder (CSV)…** lists files left for a person to handle. **Undo the last import** removes the ties and puts the old folder names back.
