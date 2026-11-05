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

## How a folder knows its client

Every client folder holds `_Vibe\client.json`. This small file ties the folder to the client, so the folder can be renamed in Explorer and Vibe Files still knows whose it is. Do not delete or edit it.

## Relationships

Select a client in Browse to see its panel: **ID**, **FOLDER**, **ENTITY TYPE** and **RELATIONSHIPS**. A relationship links an entity to an owner, for example `Baker Farms (1004)` and a partner who owns 40%.

1. Open **New relationship…**.
2. Choose **This client is the entity** or **This client is the owner**.
3. Type the other client's ID (**Owner client ID** or **Entity client ID**), the **Role** (shareholder, partner, beneficiary…), the **Share %** and a **Note** if you like.
4. Press **Add relationship**.

Intake uses relationships to file copies, for example a K-1 filed to the entity and copied to each owner (see [Intake rules](help:intake-rules)). **Remove** beside a relationship takes it off.

## Importing an existing folder tree

When the cabinet already holds client folders that are not tied to clients (an older firm tree), open **Admin** › **Folder import** and press **Scan the cabinet**. Each top-level folder is matched to a client and each file gets a doc type and tax year where its name or folder says so. Review the **Bind** and **Client** columns, then press **Import N folders**. Files are not moved.

- **Match** shows how well each folder matches its client. **90–100%**: the ID and the name agree, or the same name of two words or more; these are ticked for you. **60–84%**: the same one-word name, or only a similar name; check these, then tick them. **50% or less**: two folders point at the same client, several clients have the folder's name, the ID is the client's but the name is not, or the client already has a folder.
- **Show matches from N% up** lists only folders matched at least that well. **Hide already bound** and **Hide folders with no match** shorten the list further.
- **Tick all shown** and **Untick all shown**, or the **Bind** box at the top of the column, tick or untick every folder listed. Click one folder's box, then Shift-click another to tick or untick all the folders between.
- Ticked folders the filter hides are still imported; the button then says how many, for example **Import 12 folders (3 not shown)**.

- Tick **Rename bound folders to the firm template's name ("{Client} ({ID})"); files inside are not moved** to give them the template's folder name.
- **Export the remainder (CSV)…** lists files left for a person to handle.
- **Undo the last import** reverses it.
