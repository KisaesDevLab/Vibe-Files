# Delivering a tax return on the portal

Send a client their return as a card on the portal's home page, "Your 2025 tax return", that walks them through its steps. The portal logs everything the client does, and files a PDF of each finished step back into the client's folder.

Up to five steps: **Sign your letters**, **Review your tax return**, **Choose your package**, **Pay your fee** and **Sign your e-file authorization (Form 8879)**. See [Letter templates](help:letter-templates) for the letters and [Pricing plans](help:pricing-plans) for packages and the fee.

## Turning it on

Two switches, both off until the firm ticks them:

- On the portal, an admin ticks **Turn on tax return delivery** in **Settings › Tax return delivery**. The same page sets what a new delivery locks by default, and how long a finished card stays.
- In Vibe Files, **Admin › Portal**: tick **Tax return delivery** and press **Save**. This adds the doc types **Delivery — return record**, **Delivery — 8879 record** and the others, filed in `Income Tax/{Year}/Delivery`.

## Sending a delivery

1. Select the client in Browse and press **Deliver tax return…** in the client's details.
2. Check the **Tax year**, **Return type** and **Card title**.
3. Tick **Tax return PDF** and choose the return from the client's PDFs in the cabinet; the one for the year whose name names the return is chosen for you. Sending shares it on the portal with everyone of the client (B2 first, as **Share on portal…** does).
4. Tick **Form 8879** and paste the client's **UltraTax eSignature link** (it starts with `https://`).
5. Press **Send to client**. With **Tell the client** ticked, the client's people get an email and text naming the card, never its content.

One delivery is open per client, tax year and return type. Opening the window again shows it; change it and press **Save changes**.

**Remind the client** every few days while a step waits on them, and **Assigned to**: the staff member told when the client says they signed, when each step is done, and of every payment. Nobody assigned: the admins.

## A revised return

Choose another return for a delivery whose client already opened the first, and press **Save changes**: the window asks first. **Send the revised return** shows the card as Updated and reopens the return and Form 8879 steps; paste a new eSignature link if UltraTax made one. The letters, package and fee stand.

## What the client does

- **Review your tax return**: **View** or **Download**. The first time they do, the step is done.
- **Sign your e-file authorization (Form 8879)**: **Open eSignature** opens the link in a new tab; after signing there they press **I've signed it**, and the step says "Waiting for the firm to confirm".

A step can be locked until an earlier one is done (the portal's defaults: the return and Form 8879 after the letters, Form 8879 after the fee). A locked return is not shown anywhere on the portal: not in the client's files, not as a download, not in a link they send.

## Confirming Form 8879

When the client says they signed, the assigned staff member (or the admins) gets an email, and the window says **The client says they signed it**. Check UltraTax, then press **Confirm signed**. When every step is done the card says so and stays on the client's page for the days the firm set.

## The step PDFs

Each finished step makes a PDF: a record page (for the return, its file name, size and SHA-256, not a second copy; for Form 8879, the e-signature service it was signed on) and a separate log page of everything that happened in that step, with times, internet addresses and browsers. They arrive in the Portal inbox like a client's upload, named `2025 Delivery 2 - Tax return.pdf` and `2025 Delivery 5 - Form 8879.pdf`, and Intake files them in the client's `Income Tax/2025/Delivery` folder with their doc types (client visible). Intake's rule for these files is there only while **Tax return delivery** is on in Admin › Portal. A step done again files ` (2)`.

**Download report…** saves the whole delivery, every step and event, as one PDF outside the cabinet.

## Deliveries list

**Admin › Portal › Deliveries** lists every open delivery and agreement with the step it waits on and how many days it has been open; click a client to open its window. **Read again** reads the portal now.

- **Show** narrows the list by the step waiting (Tax return, Form 8879, Letters to sign, Package to choose, Payment due), by kind (Tax returns, Agreements), or to **Monthly charge failed**.
- **Show finished** adds those finished in the season you pick: that tax year's returns and the agreements covering them, with the day they were done.
- **Waiting too long after** sets the days after which an open item's count turns amber; the number is kept for every PC. The line above the list counts them.
- **Remind ticked** tells each ticked client again, as Remind now does. Tick rows by hand, or use **Tick all shown** or **Tick waiting too long**; only rows waiting on the client can be ticked. The line after it says who was reminded and who was not (nobody on the portal yet, or reminded 3 times that day already).
- **Monthly charge** shows an agreement's monthly amount, or the day its last charge failed.

## Where the record is kept

The delivery lives on the portal. Vibe Files reads the open deliveries every few minutes on every PC (the client's details show "Tax return delivery: 2025 1040: 1 of 2 done") and keeps a copy of each in the client's folder, `_Vibe\delivery\<id>.jsonl`, one line each time it changes, for when the portal cannot be reached. The audit trail records sending, changes, reminders, confirming, cancelling and reports.
