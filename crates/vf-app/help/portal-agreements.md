# Annual service agreements

Send a client their engagement for the year as a card on the portal, with no tax return attached: the letters for the services they take, signed on the portal, then their fees set up, charged once or every month. The money goes into the firm's own Stripe account (see [Balance owed](help:portal-balance) for setting Stripe up).

## Turning it on

Agreements come with tax return delivery: in **Admin › Portal**, tick **Tax return delivery and annual agreements** and press **Save** (the portal needs it on too, in its Settings, and must be Vibe Portal 0.5.5 or later). Saving adds the doc types **Agreement — signed letters**, **Agreement — package selection** and **Agreement — payment record** (filed in `Correspondence\<year>\Engagement`) and **Payment receipt** (`Billing\<year>`).

Before the first agreement:

- **Admin › Portal › Agreements › Partners**: who signs for the firm, with their title. A client's agreement is signed by the partner its client CSV's Partner column names (by the value you give, such as initials, or by name), else the first.
- **Admin › Portal › Letter templates**: the letters. See [Letter templates](help:letter-templates).

## The letters

Vibe Files comes with the AICPA Annual Tax Compliance Kit's engagement letters (Forms 1040, 1120, 1120-S, 1065, 1041, 990, 990-PF, 709, 706, 5500, FinCEN 114, amended returns, tax projections, tax consulting, examination representation), its Terms and Conditions Addendum and its engagement closure letter, and letters for bookkeeping, payroll and advisory services. All are marked **Sample** until you save your own text: have your attorney review them.

Each letter says `{Terms and Conditions}` where the firm's Terms and Conditions go, so one signature covers both. Edit the Terms once and every letter uses them.

The kit's fill-ins, such as `[X]%`, `[State]` or `[Frequency]`, are highlighted in yellow. **Vibe Files never sends a letter that still has one**: it names the letter and the fill-in. Fill them in the template (or turn a paragraph off with **Only for**).

## Sending one

1. Select the client in Browse and press **Service agreement…** in the client's details.
2. Check the **Agreement year** (from October, next year). The returns it covers are the year before.
3. Tick the returns and services the client takes, and type each one's **One-time** and **Monthly** fee. The total shows below.
4. **Letters to sign** lists the letters for those services, ticked as their templates say; the §7216 consents are offered unticked. **Preview letters…** shows them as the client will.
5. **Signed by**: the client's Portal contacts. A business (its entity type in the client's details) is signed for by one person with a title; anyone else by the taxpayer and spouse.
6. **Payment**: **Pay on the portal**, or **Sign only** to collect signatures alone.
7. Press **Send to client**.

**Fill from last year** copies the services and fees of the client's last agreement.

## What the client does

The card has two steps: **Sign your letters**, then **Set up payment**. The payment step shows each service, what is paid today (the one-time fees and the first month) and what every month after. They pay by bank account or card on Stripe's page. Monthly payments run **until either of you ends the agreement**.

The client's home page then shows **Your monthly payment** with **Update payment method**. When a month fails, the assigned staff member (or the admins) is told, every PC says so once, and the client's page says the last payment did not go through.

## Next year

Send next year's agreement as before. When the client signs it, their monthly payment changes to the new amount from the next charge; nothing is set up again. Only new one-time fees are paid on the card.

**Admin › Portal › Agreements** sends next year's agreements to many clients at once, each from their last agreement. Tick the clients (**Search** narrows the list), choose the group size and pause, and press **Send**; **Stop** ends a run. A letter with a fill-in left stops the run before anything is sent.

## Changing or stopping the monthly payment

In the client's **Service agreement…**: **Change from the next charge** sets a new monthly amount; **Stop monthly charge…** ends it. The client is told either way. A cancelled agreement does not stop a payment already running.

## Ending an engagement

**End engagement…** in the client's details sends the engagement closure letter. Type the letter's fill-ins for this client (the engagement's name, dates, your phone), press **Preview…**, then **Send and end engagement…**. The letter is shared with the client's people on the portal, to read, and put in the Portal inbox for Intake to file. The client's open agreements are cancelled and the monthly payment stops.

## Tax return delivery

When the year's agreement covering a return is signed, **Deliver tax return…** for that return says so and leaves **Letters to sign** unticked. Tick it to send letters anyway.

## Filed for you

Each finished step's PDF, with its log, arrives in the Portal inbox and Intake files it in `Correspondence\<year>\Engagement`. Each paid month's receipt is filed in `Billing\<year paid>`. The Deliveries page lists agreements with a **Kind** column; clicking one opens the client's agreement.
