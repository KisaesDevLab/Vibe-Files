# Sharing files with clients through your portal

How Vibe Files works with your firm's Vibe Portal: sending documents to clients, choosing who sees each one, inviting people, and receiving the files clients upload.

## What the portal is

Vibe Portal is your firm's own client portal, installed on your Vibe appliance at its own address (for example `https://files.yourfirm.com`). Clients sign in there with a link sent to their email or a code sent by text. The portal keeps an encrypted copy of what you share, and your cabinet keeps the original. An admin can move the portal's files to a B2 bucket of the portal's own (staff page, **Settings › Storage**), encrypted first; **Test** in **Admin › Portal** says where they are.

Vibe Files talks to the portal only when it is set up, and only at the address you give it.

## Setting it up

In **Admin › Portal** (admin password needed):

1. **Portal address**: the address clients use.
2. **Portal key**: a key made on the portal's staff page (`https://<portal address>/staff`, **Settings › Portal keys › Make a key**) or on the appliance with `sudo vibe portal key create "Office"`. It is shown once; paste it whole (it starts with `vpk_`).
3. **Fetch client uploads every**: how often Vibe Files collects what clients sent (5 minutes unless you change it; 0 means only when you press **Fetch uploads now**).
4. Press **Test**, then **Save**.

The address and key are kept in the cabinet (`_Vibe\keys\portal.json`, never sent to B2), so every PC that opens the cabinet uses them. **Stop using the portal** removes them for every PC; the portal keeps its files.

Saving also adds the **`_Portal inbox`** folder to Intake's shared inboxes. Press **Reload Intake** when Intake asks.

Anyone can collect client uploads without the admin password: the **Intake** tab's **Portal uploads** line has its own **Fetch uploads now** button, and says when this PC last fetched and what came of it (files received, nothing new, or a problem).

## Inviting people

Select a client in Browse and press **Portal users…** in the client panel. **Invite someone…** takes an email, a mobile number, or both; the invitation goes to the email when there is one, else by text, and works for 7 days. A mobile number given with an email is not used until the person confirms it: when they accept, the portal offers to text them a code, and once they type it back they can sign in by text too. People invited by email can also add a mobile number on the portal's home page (**Sign in by text**) when the portal sends texts. **Resend** sends a new link (the old one stops working); **Withdraw** cancels it.

A client can have several people (a spouse, a bookkeeper), and one person can have several clients; they switch between them on the portal. **Turn off** ends a person's access to that client at once.

For someone already on the portal, use **Someone who already uses the portal…**: search part of their name, email or mobile, then press **Invite** beside them. They keep one login. To invite many clients at once from the client CSV, see [inviting from the client list](help:portal-invite-list).

People can also ask for access on the portal's sign-in page. Their requests appear in **Admin › Portal requests** once they have confirmed their email or phone: see [checking who asks for access](help:portal-requests).

## Sharing documents

Select one or more documents of the same client in Browse and press **Share…** in the pane on the right. The window shows:

- each document's state: not on the portal, on the portal since a date, or **changed since it was shared**;
- the client's portal people, each with a tick: ticked people see the documents; for one document, when each person last opened it;
- people invited who have not accepted yet: ticked, they get the documents when they accept. **Resend invitation** sends a new link on the same **Save** (an expired one is resent when ticked).

Tick or untick people and press **Save**. A document is sent to the portal the first time someone is chosen to see it. When the cabinet's copy changed since it was shared, **Update on portal** sends the newer version: the client sees the same file, marked Updated and new again.

**Send invitation and share** at the bottom invites someone new and shares these documents with them.

**Remove from portal** takes the documents off the portal for everyone. The cabinet keeps them.

Clients are told by email or text when files are shared, in one message per few minutes that does not name the files.

A document on the portal shows **Share… (on portal)** in the pane. To send one to someone outside the client, see [secure links](help:portal-secure-links).

## Receiving files from clients

What clients upload is collected into the **`_Portal inbox`** folder of the cabinet, named `<Client name>_<Client ID>_<their file name>`, so Intake files it like a scan: the client is read from the name. Only one PC collects each upload. iPhone photos arrive as JPG.

**Admin › Portal** lists what was received lately, who sent it and their note, and [what is not filed yet](help:portal-inbox).

## In the audit trail

Vibe Files records each portal action it takes (settings changes, never the key; sharing, updates, removals, invitations, access changes, approvals and denials; each file received). The portal keeps its own full history, including each time a client opened a file: **History** on its staff page.

## The portal's staff page

Staff can also work at `https://<portal address>/staff`, signing in with their email, password and an authenticator app code: from home or a PC without Vibe Files. There they can invite and turn off people, approve access requests, hide or remove shared files, download what clients sent (it still arrives in `_Portal inbox`), and change the firm's name, logo and upload limits, the portal keys and the staff accounts. Sharing new documents is done only from Vibe Files. More: [documents in B2](help:portal-b2-list).
