# Similar clients

How to find clients that were made twice and merge them together, in **Admin** › **Similar clients**.

## What counts as similar

Two clients are listed as a pair when:

- **Same ID**: their IDs are the same apart from capitals, spaces, dashes, underscores and leading zeros (`AC-20` and `ac20`, `01001` and `1001`). Two clients can never have exactly the same ID.
- **Name**: their names match 97% or better, the way Intake compares names. `Smith, John` and `John Smith` are the same name.

A name match with two different IDs may be two people of one name, or two businesses of one family. Look before you tick.

## Finding the pairs

1. Open **Admin** and click **Similar clients** under **FIRM**.
2. Click **Find similar clients**. Every client is compared with every other; nothing is changed.
3. Use **Show** to see only the same-ID pairs or only the name matches, and **Name or ID contains** to narrow the list.

Each row shows both clients with their ID, how many documents each has, and whether the client CSV lists it.

## Which one is kept

The client whose ID is in the client CSV that Intake uses is the one kept. When both are in the CSV, or neither is, or no CSV is set, click **Keep** for the one that stays. A row cannot be ticked until one is chosen, and the kept client must have a folder.

## Merging

Nothing is ticked for you.

1. Tick the pairs that are one client. **Tick the ones shown** ticks every pair shown that already has a client to keep.
2. Click **Merge the … ticked…**. A window lists what each merge would do: how many documents move, how many have a name already taken in the kept client's folder (both are kept, the one moved with ` (2)`), and any pair that cannot be merged, with the reason.
3. Click **Yes, merge**. A count shows how far it is; **Cancel** stops after the pair being merged.

Each pair is merged the same way as **Merge into another client…** in Browse: the documents move into the kept client's folder, the merged client leaves the client list and the client CSV, and its relationships and legal holds become the kept client's. A pair that is refused does not stop the others; the page lists it with the reason.

A merge is refused when the client's folder holds files the catalog does not have yet, or documents that are no longer on disk. Open the client in Browse, press **Refresh**, then find and merge again. Wait for the first index of a cabinet to finish before merging.

## Putting a run back

After a run, **Undo these merges** puts the merged clients back, the last one first: their folders, documents, place on the client list and in the CSV. It is offered until you leave the program or run another merge; after that, only the cabinet's last merge can be undone, from Browse's **Merge into another client…** window.

## Not the same

Click **Not the same** on a pair that is two clients. The pair is not listed again, on any PC. **Show the … marked not the same** forgets every such mark.
