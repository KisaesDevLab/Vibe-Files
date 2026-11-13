# Weak matches and two folders for one client

How to sort out the folder import's uncertain rows: folders matched only weakly to a client, and two folders that point at the same client (such as `3 Way Construction LLC` and `3-Way Construction, LLC`).

## Weak matches become clients of their own

Set **Show matches from** to a percentage, for example 90. When some folders are matched to a client below it, the page offers **Create a client for each folder matched under 90% (N)**. Each such folder then gets a new client named after the folder, with an ID made from its name, instead of the client it only looks like. Change an ID in the box on its row and press Enter. Folders sharing a client are left out: sort those out first (below).

To look at them first, choose **under** next to **Show matches** (instead of **from**): only folders matched under the % are listed, with those matching nothing. **Clear the match on N folders shown** then removes their client, and **Create a client for each unmatched folder** makes each one a client named after it. A sure match is never shown there, so it is never cleared.

To make one folder a client of its own, use **New client** on its row when it shares a client, or pick **(none)** in its **Client** box and press **Create a client for each unmatched folder**.

## Two folders for one client

Matching ignores spaces and punctuation, so `3 Way` and `3-Way` find the same client. A client can have only one folder, so both rows wait with **2 folders point at this client**.

Tick **Only folders sharing a client** to list just these folders. On each row:

- **Merge**: tick it on the folders to combine (on one is enough when the other is bound or ticked to import). The folder kept is the client's bound folder, else the one with more files; its row says **kept: the others merge into it**, the others **merges into …**. At **Import**, their files move into the folder kept, into the same sub-folders (a name already there gets ` (2)`), and the emptied folders are removed. A folder still holding something else, such as a `_Vibe` folder, stays.
- **New client**: the folder becomes a client of its own, named after it.

**Undo the last import** puts merged files back in their own folders. It is refused for a merged file changed since, so OCR does not rewrite those files until the next import.
