# Sending a document to someone outside the client

How to send one of a client's documents to a lender, a lawyer or another accountant by a private link from your firm's Vibe Portal, without giving them a portal login.

## Sending it

Select the document in Browse and press **Send secure link…** in the pane on the right (shown when the portal is set up). Fill in:

- **Their email**: the link and the code to open it go there.
- **Their name** and **Organization** (optional): the email greets them by name.
- **Message** (optional): added to the email.
- **Days the link works**: 7 unless changed (1 to 30).
- **Allow download (not only view)**: ticked unless changed. View only works for a PDF or a picture the page can show. The firm's portal limits the downloads of each link (5 unless an admin changed it in the portal's settings).
- **Watermark PDFs with their name**: ticked unless changed. A PDF that cannot be watermarked (locked with a password, damaged) is refused when you send.

Then tick **The client has consented to sending this to this person** and press **Send**. Tax return information may be given to someone else only with the client's consent (IRS section 7216): the tick is asked for every link, and the audit trail records it with your name, the recipient and the expiry.

## What happens

1. When the document is not on the portal yet, Vibe Files sends it there first, shown to none of the client's portal people. When the portal's copy is older than the cabinet's, the newer content goes up first, so they get the current file. If the client's people see that copy, the window says so and leaves it as it is unless you tick **Send the cabinet's newer version** (they then see it too, and are told it was updated); left unticked, the link gives the portal's older copy.
2. The portal emails the link. The email does not name the client or the file.
3. The recipient opens the link and asks for a code; it is emailed to the same address. With it they can view (and download, when allowed) for 30 minutes at a time, until the link expires.

If the email cannot be sent, no link is made. When the document was sent to the portal before something failed, it stays there, shown to none of the client's people, and **Send** again does not send it twice. A document on the portal only for a link is not marked **(on portal)** in Browse. **Remove from portal** in **Share…** also stops every link to it.

## Seeing and ending links

The links are listed on the portal's staff page, on the client's page under **Secure links sent outside**: who each went to, when it was opened and how many downloads. **Revoke** stops one at once. The **Portal** button in the header opens the staff page's sign-in. The portal's **History** also records each link, with the Windows user who sent it from Vibe Files.

One recipient can hold 5 working links for one client; each Windows login can send up to 50 a day from each PC. This needs a Vibe Portal released after 0.3.6 that has it (Vibe-Portal P68): an older portal says to update it.
