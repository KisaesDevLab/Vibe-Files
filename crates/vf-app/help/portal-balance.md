# Balance owed on the portal

Post an amount a client owes, and the client pays it on the portal by card or bank account. The money goes into the firm's own Stripe account; no card or bank details reach the portal or Vibe Files.

## Turning it on

Two switches, both off until the firm ticks them:

- On the portal, an admin sets up Stripe in **Settings › Payments**: the Stripe secret key, the webhook's signing secret, the methods offered, any card fee, and **Turn on balance cards**.
- In Vibe Files, **Admin › Portal**: tick **Balance owed** and press **Save**. This adds the doc type **Payment receipt** (folder `Billing/{Year}`) when the cabinet does not have it.

## Posting a balance

Select the client in Browse and press **Balance owed…** in the client's details. Fill in:

- **Amount**, in dollars and cents (at least $0.50).
- **Memo**: what it is for, such as "2025 bookkeeping, Oct–Dec". The client sees it, and the email and text about the balance say the amount and the memo.
- **Assigned to**: the staff member told when it is paid, started, failed or expired. Nobody: the person named in the portal's Settings › Payments, else the admins. A new balance (and a new Deliver… or Service agreement… card) starts with the person chosen in Admin › Portal's **Assign new cards to**, when they can see the client.
- **Remove from the portal after N days**: the card leaves by itself on that day if unpaid. Untick it for no limit.
- **Tell the client by email and text**, and **Remind** every few days while it is due.
- **Hold the client's files until this is paid**: while it is due, the client sees their shared files listed, marked *Opens when your balance is paid*, with the **Pay** button, but cannot open, download or share them; files shared meanwhile wait too. Documents of a delivery whose fee is paid stay open (Vibe Portal 0.5.8 or later). Paying (a bank payment started is enough), **Clear balance**, or the card leaving after its days releases them. Their uploads to you still work. Needs Vibe Portal 0.5.6 or later: an older portal posts the balance without the hold, and the window says so.

Press **Post to portal**. The client's home page shows a **Balance due** card with one **Pay** button (two, bank account and card, when the firm charges a card fee, so the fee is seen first).

To change the amount or memo, open the window again and press **Save changes**. **Clear balance** takes the card off the portal. **Remind now** tells the client again (three times a day at most).

## What happens next

- **Paid by card**: the balance is paid at once. The client's card says Paid with a receipt for the days the firm set, then leaves.
- **Paid by bank account**: the card says **Payment started** until the bank confirms, usually a few business days. A second Pay is refused meanwhile. If the payment fails, the balance is due again (or expired, if its date passed).
- **A new amount while a payment is pending**: it is posted as a new balance, due once the pending payment settles. The window says so.
- **Cleared while a payment is pending**: the payment is still followed, and its receipt is still filed if it clears.
- **Past its date, unpaid**: the card leaves the portal. Every PC says so once, and the Balances list shows it as expired that month.

Vibe Files reads the balances every few minutes on every PC: the client's details show "Balance owed: $240.00 on the portal, 9 days left", "payment started", "paid" or "none".

## The receipt

When a balance is paid the portal makes a **Payment receipt** PDF: what was paid, how, the card fee if any, and a log page of everything that happened to the balance (posted, the client seeing it, checkout, paid), with times, internet addresses and browsers. It arrives in the Portal inbox like a client's upload, named `<date paid> Payment receipt - <memo>.pdf`, and Intake files it in the client's `Billing/<year paid>` folder with the doc type Payment receipt (Intake's rule for it is there only while **Balance owed** is on). The client's own receipt has no log page.

### A receipt printed at the office

The portal can also print a receipt at the office when a client pays online, through Vibe Print, the appliance's print gateway: a receipt slip, or the letter receipt on an office printer. This covers balances, tax return fees, agreement payments and monthly charges. It is set up on the portal's staff page, **Settings › Printing**, not in Vibe Files, and prints even when no Vibe Files is open. A payment you mark paid in Vibe Files does not print.

## Balances list

**Admin › Portal › Balances** lists every client's balances: those due, with a payment started, paid this month and expired this month. Click a client to open its window. **Read again** reads the portal now.

## Where the record is kept

The amount lives on the portal. Vibe Files keeps a copy of each balance it reads in the client's folder, `_Vibe\balance.jsonl`, one line each time a balance changes, for when the portal cannot be reached. It is never sent to B2. The audit trail records posting, changing, clearing, reminding, paid and expired, with no amounts.

Refunds are made in Stripe and are not shown here.
