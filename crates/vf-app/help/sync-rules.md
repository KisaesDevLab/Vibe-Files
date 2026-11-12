# What goes to B2

How Vibe Files decides which documents are sent to your Backblaze B2 bucket, to keep the bucket (and its cost) to the files you still work with, and what happens to the rest.

## The rule

A document that has never been sent goes to B2 when:

- its tax year is this year or one of the last few (3 unless changed: in 2026, tax years 2023 and later); or
- it has no tax year and is in a client's **Permanent** folder (formation papers, EIN letters, elections).

Other documents that were never sent, such as old tax years, or files with no tax year outside a Permanent folder, are not sent. They are **held back**.

Anything already in B2 keeps syncing whatever the rule says: an edit, a move or a deletion still goes up, so the bucket never shows an old version as current. Nothing is ever taken out of the bucket by the rule.

## Changing it

Open **Sync** › **What goes to B2**. Type the number of tax years before this one to send in **Tax years before this one to send (0 = every year)** and press **Save** (this needs the admin password when one is set, like the other sync settings). 0 sends every document. Widening the rule makes the held-back documents pending, and the next push sends them.

The setting is the cabinet's, so every PC (and every office that pulls from the bucket) follows it. Update every PC to the same version of Vibe Files: an older one sends everything, and when it saves client details it drops these settings (a newer one puts them back from its own copy, `_Vibe\b2-rules.json`).

## One client's own choice

In Browse, select a client: **B2 SYNC** in its panel is **The firm's rule** unless changed. **Always send** sends every document of that client, whatever its year. **Never send** holds back every document of that client not yet in B2 (what is already there keeps syncing). Anyone can change it.

## What is held back

**What goes to B2** shows how many documents are held back now, and their size. They are not in B2: make sure the share itself is backed up another way. A document with no tax year that should go up can be given its year (Browse › **Needs tax year**), or moved to the client's Permanent folder.
