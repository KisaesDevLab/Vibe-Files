# What goes to B2

How Vibe Files decides which documents are sent to your Backblaze B2 bucket, how to send one by hand, and what happens to the rest.

B2 is mainly for sharing documents with clients through the portal, so little goes up by itself. The share that holds the cabinet needs its own backup.

## The rules

A document that has never been sent goes to B2 when **any** rule in the firm's list matches it. A rule says:

- **Folders**: the folders of a client folder it covers, such as `Income Tax` or `Permanent/Setup`. A document anywhere below one of them matches. None ticked means any folder.
- **Doc types**: the doc types it covers. None ticked means any doc type, and documents with none.
- **Tax years**: any tax year, this year and a number of years before it, or none with a tax year.
- **Include documents with no tax year**: whether a document with no tax year matches too.

Documents no rule matches are not sent. They are **held back**.

Anything already in B2 keeps syncing whatever the rules say: an edit, a move or a deletion still goes up, so the bucket never shows an old version as current. Nothing is ever taken out of the bucket by a rule.

A cabinet that has never changed its rules shows two: **Recent tax years** (this year and the three before, any folder and doc type) and **Permanent files** (documents with no tax year in a Permanent folder). They are what was sent before there was a list.

## Changing them

Open **Sync** › **What goes to B2**. This needs the admin password when one is set, like the other sync settings.

1. Click **Add rule**, or **Edit** beside a rule.
2. Type a **Rule name**, tick the folders and doc types, choose the tax years.
3. Click **Save rule**.

**Delete** takes a rule off the list; what it sent stays in B2. With no rules, nothing goes up by itself.

The line below the list counts what is held back now. A new or wider rule makes the documents it matches pending, and the next push sends them.

The list is the cabinet's, so every PC follows it. Update every PC to the same version of Vibe Files: an older one does not know the list.

## Sending documents by hand

Tick documents in Browse and choose **Sync to B2** › **Push the selected documents now**. They go to B2 at once, whatever the rules hold back, and keep syncing afterwards like anything else in B2. Nothing else goes with them.

Only documents in a client folder can be sent this way.

Sharing a document through the [portal](help:portal), or sending a [secure link](help:portal-secure-links), sends it to B2 first in the same way. If B2 sync is locked on the PC, or the push cannot finish, the share is stopped and says why. A cabinet with no B2 sync shares as before.

## Taking documents out of B2

Tick documents in Browse and choose **Sync to B2** › **Remove the selected documents from B2…**, then confirm. They are deleted from the B2 bucket only:

- The files stay in the cabinet on every computer.
- A portal link to them stops working.
- No rule sends them again. Pushing or sharing them by hand puts them back.

Anyone can do this; it is written to the audit trail. If another computer's changes to the client are not in yet, the documents leave B2 at the next push instead.

## One client's own choice

In Browse, select a client: **B2 SYNC** in its panel is **The firm's rule** unless changed. **Always send** sends every document of that client. **Never send** holds back every document of that client not yet in B2 (what is already there keeps syncing, and a document pushed by hand still goes). Anyone can change it.

## What is held back

**What goes to B2** shows how many documents are held back now, and their size. They are not in B2: make sure the share itself is backed up another way.
