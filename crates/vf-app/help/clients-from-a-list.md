# Clients from a list

How to put your firm's list of client names and IDs onto the clients in the cabinet: each matched client takes the list's ID and name, and its folder is renamed to match.

Use this when the clients in Vibe Files came from folder names and carry old, wrong or missing IDs, and you have a list from your tax software with the right ones.

## The list

A CSV file with two columns: the client's name and its ID. The first row holds the column names. Full SSNs or EINs are refused as IDs.

## Finding the matches

1. Open **Admin › Clients from a list**.
2. Press **Choose…**, pick the CSV, then **Read the list**.
3. Check **ID column** and **Name column**. Vibe Files guesses them; change them if the guess is wrong.
4. Leave **Names match at least** at 97 %, or change it. 97 % allows a missing comma, or `Inc` for `Inc.`. A lower number finds more matches, and more wrong ones.
5. Press **Find matches**. Nothing is changed.

Each row of the list is matched to one client:

- by its ID, when a client has that ID already (only the name's spelling changes);
- otherwise by name, to the one client whose name agrees closely enough.

## What the list shows

- **Name 98%**: the names match and the client has no ID yet.
- **Name 98% · has another ID**: the names match, but the client has a different ID. Two people of one name are not one client, so look before you tick.
- **Same ID · name 96%**: the client has this ID; the list spells its name another way.

Use **Show** and **Name or ID contains** to narrow the list. **Tick the ones shown** ticks every match shown, but clients that have another ID are ticked only when **Show** is set to **Has another ID**.

Under the counts, **Some are not changed: more than one match** lists what Vibe Files will not touch:

- a row that matches two or more clients. These are usually one client made twice: merge them in [Similar clients](help:similar-clients) first, then find again;
- a client that two rows of the list match;
- an ID that is on the list more than once.

Rows that match no client are not added here. **Browse › Import clients CSV…** adds the list's clients.

## Updating

1. Tick the matches that are right.
2. Press **Update the N clients ticked…**. A window shows each change and each folder's new name, and any that cannot be made (for example, the ID belongs to another client).
3. Press **Yes, update**. A count shows the progress; **Cancel** stops after the client in hand.

For each client, the ID and name change on the client list, in the catalog and in the client's folder, and the folder is renamed, for example `Smith, Pat (SMIT1001)`. The documents stay in their folders.

After an update the changed clients leave the list. The list is not matched again until you press **Find matches**.

## Undo

**Undo the last update** gives the clients the ID, name and folder they had, the last one first. It works from any PC, also after Vibe Files was closed: press **Find matches** to see the button. A client that was changed again since is not put back, and the undo stops there. The client CSV that Intake uses is left as it is.

> A large update takes a while: each client's folder is renamed on the share one at a time. Leave Vibe Files open until the count reaches the end.
