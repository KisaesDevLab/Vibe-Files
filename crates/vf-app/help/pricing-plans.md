# Pricing plans

The packages and add-ons a tax return delivery offers the client, per return type and tax year. Open **Admin › Portal › Pricing plans**.

## A plan

Press **New plan…** to start from the sample, or choose a plan on the left. Set the **Return type**, the **Tax year**, and the **Base fee name** (used when a delivery offers a base fee and add-ons).

**Packages**: one row per package, the first being the return fee itself (Tax Return Only). For each:

- its name, and **Recommended** for at most one (a tag the client sees; nothing is chosen for them);
- **over the return fee**: a percent, and **at least** an amount. A package costs the client's return fee plus that percent of it, but at least the minimum. The sample: Advantage 10 % (at least $127), Concierge 35 % (at least $443). On a $1,265.68 return fee both minimums apply: $1,392.68 and $1,708.68;
- its services, a group a line: `Notices: IRS and state letters answered; Audit representation`. The client compares the packages by these lines.

**Add-ons**: name, what the client reads, price, and **Offered unless unticked**.

**As the client sees it** works the prices out for a return fee you type. **Save** keeps the plan; **Copy to next year…** makes next year's plan from this one; **Delete** removes it.

## In a delivery

In **Deliver tax return…**, tick **Package and fee** and choose the mode:

- **One amount**: the fee, typed.
- **Base fee and add-ons**: this client's return fee, and the add-ons to offer.
- **Packages and add-ons**: this client's return fee; each package's price is worked out from the plan. You can untick a package or type another price for this client: a price that differs from the plan is outlined, with the plan's price beside it.

When the delivery is sent it keeps these prices. Editing the plan later changes new deliveries only.

A package with an addendum template (Admin › Portal › Letter templates, kind Package addendum) sends that addendum with the delivery: the client reads it and signs it with their choice. On the portal the client chooses a package and add-ons, signs, then pays the total on Stripe's page, through the firm's own Stripe account (see [Balance owed](help:portal-balance) for setting Stripe up). A bank payment shows "Payment started" until it clears; if it fails, staff are told, every PC says so once, and the fee shows unpaid again.

**Mark paid…** records a fee paid another way, with a note such as a check number.

The selection's PDF (step 3) and the fee's receipt (step 4), each with its log page, are filed in the client's `Income Tax\<year>\Delivery` folder.
