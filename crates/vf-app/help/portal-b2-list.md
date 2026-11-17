# The documents in B2 on the portal's staff page

How staff can see, on your firm's portal, which clients have documents in B2 and what those documents are.

## What it does

Vibe Files encrypts what it sends to B2 with your sync passphrase, so the bucket alone shows nothing. So that staff can see on the portal which clients have documents in B2, and what they are, Vibe Files can send the portal a list. The list holds names only. The portal opens a document in B2 only if your firm gave it the sync passphrase (see below).

## Turning it on

It needs Vibe Portal 0.3.15 or later, and Vibe Files 0.1.38 or later on every PC. Update the portal first, then every PC, then tick the setting.

In **Admin › Portal**, tick **Show the documents in B2 on the portal's staff page** and press **Save**. Then:

- The computer B2 sync runs on sends each client's list every 10 minutes while Vibe Files is open there, and only when it changed: each document's place in the client's folder, its size and tax year. **Vibe Files sends no document this way**, only these names.
- On the staff page, **Clients** shows every client with documents in B2, and a client's page lists them under **Documents in B2 (from Vibe Files)**, saying which are on the portal. Clients' people see nothing of this.
- Only what is in B2 is listed. A document held back by [What goes to B2](help:sync-rules) is not; push it by hand or change the rule to list it.
- The list follows B2 sync: a document appears after it has been pushed, and leaves after it is removed from B2 or deleted.

Where every PC runs B2 sync itself (a cabinet on a plain shared folder), each of them sends, and the portal keeps one list for each client: the newest.

## Adding a document to the portal from the staff page

With Vibe Portal 0.3.16 or later, an admin can enter your **sync passphrase** on the portal's staff page (**Settings › Storage**, with the B2 bucket Vibe Files syncs to). Nothing in Vibe Files changes, and nothing is ticked here. From then on:

- A client's page on the staff page has **Add to portal** on each document in its list. The portal reads the document from B2 itself and keeps its own copy, shown to nobody until staff choose who sees it.
- **The portal's appliance can then open every document Vibe Files has in B2**, not only the ones added. Until the passphrase is entered there it can open none. Leave it out if that matters more; sharing from Vibe Files works either way.
- A document added that way follows B2 (Vibe Portal 0.3.18 or later): it is replaced on the portal by itself when a changed version is pushed, and **removed from the portal when it is deleted here or taken out of B2**. One shared from Vibe Files changes only when it is shared again from here, and stays until it is removed there.
- Vibe Files finds such a document as one on the portal: Browse marks it, and sharing it from here does not add it twice.

**Remove** on the same page of the portal takes the key off it again. The portal's guide says more.

## Limits

The portal lists at most 20,000 documents for one client, and a document's folder and name together at most 500 characters. A client over either gets no new list; the portal shows the last one sent, if any. Browse says which client, once.

## Turning it off

Untick it and save to stop: the lists are removed from the portal at once. **Stop using the portal** removes them too, and changing the portal address removes them from the portal used before. Clients the lists put on the portal stay there, ready to invite.

If the portal cannot be reached at that moment, the page says the lists could not be removed, and what to do.

## A new B2 bucket

When B2 sync is set up again on a new bucket, the portal's lists are of the old one. Vibe Files removes them before it sends the new bucket's lists. While the portal cannot be reached, Browse says the new lists are waiting; it is tried again every 10 minutes.

## Older versions

- An older Vibe Files does not know the setting. On the computer B2 sync runs on it sends nothing. One that saves **Admin › Portal** unticks the setting, and its **Stop using the portal** leaves the lists on the portal. To put that right from an updated PC: to keep the lists, tick the setting again and save; to remove them, tick it and save, then untick it and save.
- With an older portal, saving the setting says the portal should be updated, and nothing is sent until it is.
