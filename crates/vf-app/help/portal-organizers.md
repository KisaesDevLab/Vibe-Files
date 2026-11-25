# Organizers

If your firm sends clients an online organizer (Stanford Tax), the portal can show each client an **Organizer** card on their home page with their own link. The card goes away by itself once you start the client's tax return delivery for that year.

## Importing the links

1. In the organizer app, export the client list with the questionnaire links (a CSV).
2. In Vibe Files, open **Admin › Portal › Organizers** and press **Import CSV…**, then pick the export.
3. Check the preview, then press **Send N links**.

The preview shows each row of the export and the client it goes to, and how it was found:

- **UltraTax ID**: the export's Source id is the client's ID. Ticked.
- **Email and name**: an email in the export is one of the client's Portal contacts, and the names agree. Ticked.
- **Check**: something disagrees (the ID is one client, the email another; or the email is on several clients). Pick the client, then tick it.
- **Name only**: only the name matches. A suggestion, not ticked: tick it if it is right.
- **No match**: pick the client yourself with **Find a client** in the list.

A wrong match would show one family's organizer to another, so nothing is sent until you press Send. Only https links on the organizer's site (`clients.stanfordtax.com`) are taken; a row for another tax year is left out. A client that is not on the portal yet is added, so the card is waiting when someone from that client signs in.

Importing a newer export later updates each client's status (Not seen yet, In progress, Submitted). Rows that are the same are left alone.

## On the client's portal

The card says the tax year and the status and has **Open your organizer**, which opens the organizer in a new tab. After the client submits, the card thanks them and keeps the link, so they can look back or add a late document.

The card goes away when:

- you create the client's tax return delivery for that tax year (**Deliver tax return…**);
- the export says the return was delivered through the organizer app; or
- you turn it off.

## Turning a card off or on

- **Admin › Portal › Organizers**: tick clients and press **Turn off ticked** or **Turn on ticked**.
- **The client panel**: the line "Organizer 2025: In progress · card showing" has **Turn off** (or **Turn on**) and **Open link ↗**.
- **The portal's staff page**: the client's page has an Organizer card with the same buttons.

A card you turned off stays off when you import the export again. Turning one off hides it from the client's portal; the link itself still works in the organizer app.

## What is recorded

The portal's History records each link sent or changed and each card turned off or on, with who did it; never the link itself. The Vibe Files audit trail records each import (how many sent, added, changed) and each turn off or on.
