# Checking who asks for portal access

How Vibe Files finds the client a request for portal access is for, what counts as a match, and what Approve asks before it lets someone in.

## Where requests appear

Anyone can fill in the request form on the portal's sign-in page. A request appears in **Admin › Portal requests** once the person has confirmed their email or mobile with a code. Every few minutes each PC looks at the portal, and the footer says **N portal access requests waiting**; click it to open the page.

When the portal asks for a photo ID, the page also shows what the barcode on the back of the ID said (name, date of birth, expiry, address) and the portal's warnings about it. The pictures of the ID are on the portal's staff page.

## How the client is found

The email or cell the person confirmed is looked up in every client's **Portal contacts**: the taxpayer's and spouse's emails and cells from the client CSV, or typed in a client's panel (see [inviting from the client list](help:portal-invite-list)).

- **On file**: each client with that contact is listed and ticked, with the person's role there: Taxpayer, Spouse, or Owner for a business. Someone on file at their 1040 and their LLC is offered for both; untick any. **Approve as Spouse** (or **Approve for 2 clients**) gives them access in one click, with one login.
- **Not on file, but the name is**: clients whose taxpayer or spouse has the first and last name they typed are listed as leads, in orange. A name can be typed by anyone, so Approve always asks how you checked. **Keep … as their email** saves the contact they confirmed in that client's Portal contacts, marked typed.
- **Neither**: open **Choose the client**, pick it, type their role, and press **Approve** or **Deny**. **Other client…** does the same when the page found someone else.

## The date of birth

In **Admin › Portal**, under **Checking access requests**, choose the column of your client list with the client's date of birth (only columns ticked as a token in **Admin › Clients** are listed). A date of birth read from the ID is then compared with it. It never counts alone: a barcode can be made by anyone. When it differs from the record, Approve asks how you checked. **Dates are written** says how your list writes a date such as 6/1/1990.

A date your list writes that Vibe Files cannot read (a two-digit year, words) shows as **the record's date could not be read**: it is not compared and is not a difference.

Earlier versions chose the email and mobile columns here too; those are moved to Portal contacts once, by themselves.

## Sharing documents on Approve

Under **On Approve, offer to share documents**, tick the doc types to offer (a tax return, an engagement letter) and how many recent tax years. On a request, **Also share with them** lists that client's documents of those types: the recent ones ticked, older ones unticked. What stays ticked is shared with the person as soon as Approve is done, through B2 first when B2 sync is set up.

## What Approve asks

- On file, with nothing in the way: one click. The portal keeps what matched with the approval.
- The portal found the name on the ID different from the name on the request, the date of birth differs, the request is a lead by name, or (with the portal's **Check before approving** on) it matched nothing: Approve asks **How did you check?**

The choices are: called the number in your records (say who you spoke to), met in person, or known to you (say how). Your answer is kept with the approval on the portal and printed in its PDF. A text or email to the contact on the request is not a check: that contact is what is being checked.

Nothing from your client list is sent to the portal, only what matched.

## Two people changing the settings

If someone on another PC saves the portal settings while you have **Admin › Portal** open, your Save is refused and the page shows theirs. Make your change again.
