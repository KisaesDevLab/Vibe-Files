# Sharing files with clients through your portal

How Vibe Files works with your firm's Vibe Portal: sending documents to clients, choosing who sees each one, inviting people, and receiving the files clients upload.

## What the portal is

Vibe Portal is your firm's own client portal, installed on your Vibe appliance at its own address (for example `https://files.yourfirm.com`). Clients sign in there with a link sent to their email or a code sent by text. Nothing is stored with anyone else: the portal keeps an encrypted copy of what you share, and your cabinet keeps the original.

Vibe Files talks to the portal only when it is set up, and only at the address you give it.

## Setting it up

In **Admin › Portal** (admin password needed):

1. **Portal address**: the address clients use.
2. **Portal key**: the key made on the appliance with `sudo vibe portal key create "Office"`. It is shown once; paste it whole (it starts with `vpk_`).
3. **Fetch client uploads every**: how often Vibe Files collects what clients sent (5 minutes unless you change it; 0 means only when you press **Fetch uploads now**).
4. Press **Test**, then **Save**.

The address and key are kept in the cabinet (`_Vibe\keys\portal.json`, never sent to B2), so every PC that opens the cabinet uses them. **Stop using the portal** removes them for every PC; the portal keeps its files.

Saving also adds the **`_Portal inbox`** folder to Intake's shared inboxes. Press **Reload Intake** when Intake asks.

## Inviting people

Select a client in Browse and press **Portal users…** in the client panel. **Invite someone…** takes an email, a mobile number, or both; the invitation goes to the email when there is one, else by text, and works for 7 days. **Resend** sends a new link (the old one stops working); **Withdraw** cancels it.

A client can have several people (a spouse, a bookkeeper), and one person can have several clients; they switch between them on the portal. **Turn off** ends a person's access to that client at once.

People can also ask for access on the portal's sign-in page. Their requests appear in **Admin › Portal requests** once they have confirmed their email or phone: choose the client their files belong to and press **Approve**, or **Deny**.

## Sharing documents

Select one or more documents of the same client in Browse and press **Share…** in the pane on the right. The window shows:

- each document's state: not on the portal, on the portal since a date, or **changed since it was shared**;
- the client's portal people, each with a tick: ticked people see the documents; for one document, when each person last opened it;
- invitations not yet accepted.

Tick or untick people and press **Save**. A document is sent to the portal the first time someone is chosen to see it. When the cabinet's copy changed since it was shared, **Update on portal** sends the newer version: the client sees the same file, marked Updated and new again.

**Remove from portal** takes the documents off the portal for everyone. The cabinet keeps them.

Clients are told by email or text when files are shared with them, gathered into one message per few minutes. The message does not name the files.

A document shared on the portal shows **Share… (on portal)** in the pane.

## Receiving files from clients

What clients upload is collected into the **`_Portal inbox`** folder of the cabinet, named `<Client name>_<Client ID>_<their file name>`, so Intake files it like a scan: the client is read from the name. Only one PC collects each upload. iPhone photos arrive as JPG.

**Admin › Portal** lists what was received lately, with the client, who sent it, and their note.

## In the audit trail

Vibe Files records each portal action it takes: settings changes (never the key), sharing and unsharing, updates, removals, invitations, access changes, approvals and denials, and each file received. The portal keeps its own complete history, including every time a client opened or downloaded a file.
