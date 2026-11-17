# Clients from a list

How to put your firm's list of client names and IDs onto the clients in the cabinet: each matched client takes the list's ID and name, and its folder is renamed to match.

Use this when the clients in Vibe Files came from folder names and carry old, wrong or missing IDs, and you have a list from your tax software with the right ones.

## The list

A CSV file with two columns: the client's name and its ID. The first row holds the column names. Full SSNs or EINs are refused as IDs.

## Finding the matches

1. Open **Admin › Clients from a list**.
2. Press **Choose…**, pick the CSV, then **Read the list**.
3. Check **ID column** and **Name column**. Vibe Files guesses them; change them if the guess is wrong.
4. Leave **Names match at least** at 97 %, or lower it to see more names that are only close.
5. Press **Find matches**. Nothing is changed.

Each row of the list is matched to one client:

- by its ID, when a client has that ID and its name agrees. `01002`, `1002` and `10-02` count as one ID;
- otherwise by name. The client's name and its folder's name are both tried.

## How names are compared

Names are compared word by word, not letter by letter:

- **Same name**: the same apart from capitals, punctuation and `Last, First`.
- **Same, written differently**: the same words once what carries no meaning is set aside: `&` and `and`, `LLC`, `Inc` or `Incorporated`, a leading `The`, a middle initial on one side, `(deceased)` or the ID in a folder's name, or the words in another order.
- **Close: look**: something differs that can mean another client: a number, `Jr` or `II` on one side, two legal forms (`LLC` and `Inc`), one word more, a word shortened or a letter off. These are shown only when their percentage reaches **Names match at least**.

`Smith, John` and `Smith, Jon`, or `Baker Properties 1` and `Baker Properties 2`, are never offered as a match.

## What the list shows

The **Match** column says which of the three it is, and what the client has now: no ID yet, **has another ID** (look before you tick), or **Same ID** with the name written another way.

Use **Show** and **Name or ID contains** to narrow the list. **Tick the ones shown** ticks only what is surely one client:

- with **Show** on **All matches**: the ones with the same name;
- names written differently only when **Show** is set to **Same name, written differently**;
- clients that have another ID only when **Show** is set to **Has another ID**;
- close matches never: tick those one at a time.

## When the list was already imported

If the list was imported with **Browse › Import clients CSV…** before, each of its clients exists with the right ID and no folder, beside the same client made from its folder under an old ID. The match then shows **folder under another ID**. Updating makes the two one client: the folder, with the list's ID and name. These are ticked with the rest when the name is the same.

## What did not match

Under the counts, the **Not matched** line counts the discrepancies, and two folded lists show them with the nearest thing to each and how close it is:

- **On the list, no client**: rows no client matches. The nearest may be a folder with no client file; [Folder import](help:folder-import) makes such a folder a client.
- **On the list as a client, with no folder**: the list's client exists, but no folder in the cabinet has its name.
- **Clients not on the list**: clients no row is for.

**Save the comparison as CSV…** writes all of it (matches, both lists, folders with no client file, and what is not changed) to open in Excel.

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
