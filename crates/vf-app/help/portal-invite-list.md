# Inviting clients from the client list

Put many clients on the portal at once, using the emails and cell numbers in the client CSV from your tax software.

## The CSV's contact columns

Add the contacts to the client CSV you already import, one row per client:

- **Taxpayer first name** and **last name**: the name on the invitation (without them, the client's name is used);
- **Email** and **Cell phone**;
- for a joint return, **Spouse first name**, **Spouse last name** (blank: the taxpayer's), **Spouse email** and **Spouse cell**.

In **Import clients CSV…**, press **Read**, open **Portal contacts** and choose the column for each. Usual header names (TP Email, Cell, Mobile, Spouse Email, SP Cell) are chosen for you. Phone numbers may be written any way. **Import** keeps the contacts in the cabinet, so the CSV is not needed after; each later Import updates them, after showing what changes (see [clients](help:clients)). Intake's own re-reads of the CSV never change them.

## Typing contacts on a client

In Browse, open a client's **PORTAL CONTACTS** in the client panel and press **Edit contacts…**. What you type is marked **(typed)** and kept even when a later CSV says otherwise; the panel shows what the CSV says. **Use the CSV values** drops what was typed. A client added in Vibe Files can have contacts this way too.

## Who is invited

- A person: one invitation, role **Taxpayer**.
- A joint return: the taxpayer and the spouse each get an invitation (roles **Taxpayer** and **Spouse**), and each has a login of their own. Both see the client's files and to-do lists.
- A couple sharing one email: one invitation in both names, role **Taxpayer & Spouse**.
- A business (the client's entity type is a partnership, corporation, LLC, trust, estate or nonprofit): the contact is invited as **Owner**; spouse columns are left out.

An invitation goes to the email; a cell given with it is kept for the person to confirm when they accept. Someone with only a cell is invited by text. An email or number that is not one is said, and not used.

## Admin › Portal › Invite from client list

The page shows each client with the people the CSV names and where each stands on the portal:

- nothing said: not on the portal, ticked to invite;
- **invitation expired**: ticked, a new one is sent;
- **invited, not accepted yet** or **on the portal**: not sent again;
- in orange, **the portal already has …**: the client has other people on the portal, perhaps this person with another address. Vibe Files never changes anyone's login: tick them only if they are someone new, or tell the client.

**Show** picks clients not on the portal yet (nobody can sign in) or every client; **Partner** and **Find** narrow the list. **Tick all shown** and **Untick all** help with long lists. People in the CSV who can't be invited (no email or cell) are listed at the top.

Press **Invite N people at M clients**. Each client is put on the portal, then its people are invited.

## Groups and pauses

So that your email provider does not take a large batch for spam, invitations go out in groups of 20 with a pause of 60 seconds between groups. Both numbers can be changed above the list. While it sends, the page says how many went and when the next group starts; **Stop** ends it between two invitations.

Keep Vibe Files open until it is done. If it is closed, the rest are not sent: open the page again and the people already invited show as waiting, so nobody gets a second invitation.

At the end the page says how many were invited by email and by text, how many were already on the portal, and why any were not sent. Each invitation is in the audit trail, without the email or number.

The portal must be 0.5.2 or later.
