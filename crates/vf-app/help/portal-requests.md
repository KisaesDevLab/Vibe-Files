# Checking who asks for portal access

How Vibe Files compares a request for portal access with your client list, what counts as a match, and what Approve asks before it lets someone in.

## Where requests appear

Anyone can fill in the request form on the portal's sign-in page. A request appears in **Admin › Portal requests** once the person has confirmed their email or mobile with a code. Choose the client their files belong to, type their role if you like, and press **Approve**, or **Deny**.

When the portal asks for a photo ID, the page also shows what the barcode on the back of the ID said (name, date of birth, expiry, address) and the portal's warnings about it. The pictures of the ID are on the portal's staff page.

## Telling Vibe Files where to look

In **Admin › Portal**, under **Checking access requests**, choose which columns of your client list hold the client's email, mobile number and date of birth, then press **Save**. Only columns ticked as a token in **Admin › Clients** are listed; press **Read the client list again** after ticking one. With no column chosen, no request can match.

Nothing from your client list is sent to the portal, only which of the three matched.

**Dates are written** says how your list writes a date such as 6/1/1990. Left on **Work it out from the list**, the page says what it found. Choose **Month first** or **Day first** if that is wrong or unclear.

## What a match is

A request matches when the email or mobile the person confirmed is the one in the client's row. A cell may hold several addresses or numbers. Only US and Canadian numbers are compared.

A date of birth from the ID is shown beside the record's and is said with a match, but never counts alone: a barcode can be made by anyone.

A date in your list that Vibe Files cannot read (a two-digit year such as 1/6/90, words, or a date that could be either way round in a list that does not say) shows as **the record's date could not be read**. It is not compared, and it is not a difference.

If the client you picked is not in the client list, the page says so: there is nothing to compare the request with.

## What Approve does

- A request that matches approves in one step. The portal keeps what matched with the approval.
- If the portal found that the name on the ID differs from the name on the request, Approve asks how you checked, even when the contact matches.
- A request that does not match: when the portal's **Check before approving** is on (its staff page, **Settings**), Approve asks **How did you check?** When it is off, Approve asks nothing.

The choices are: called the number in your records (say who you spoke to), met in person, or known to you (say how). Your answer is kept with the approval on the portal and printed in its PDF. A text or email to the contact on the request is not a check: that contact is what is being checked.

The portal's other warnings (the ID has expired, its barcode could not be read, the person said it has no barcode) are shown and do not change what Approve does.

## Two people changing the settings

If someone on another PC saves the portal settings while you have **Admin › Portal** open, your Save is refused and the page shows theirs. Make your change again.
