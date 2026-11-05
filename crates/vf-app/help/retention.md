# Retention and destruction

How long documents are kept, how to review the ones whose time is up, approving or deferring them, the destruction run and its certificate, and how legal holds stop it.

## Retention classes

Open **Admin** › **Retention classes**. Each class says how many years a document is kept and from when. Vibe Files starts with these:

- **Returns & workpapers**: 7 years from the end of the tax year.
- **Client source documents**: 4 years from the end of the tax year.
- **Engagement letters**: 7 years from the engagement's close.
- **Permanent file**: kept permanently.
- **Correspondence**: 4 years from the end of the tax year.
- **Notices**: 7 years from the end of the tax year.

To change how long a class is kept, press **Edit…** on its row (or click its name); **Add class** makes a new one. Fill in **Class name**, choose **Counted** (**From the end of the tax year**, **From the engagement's close**, **From the day it was filed** or **Kept permanently**) and type the **Years**. Press **Add** or **Save**; **Remove** deletes a class.

Each [doc type](help:doc-types) picks its class in its **Retention** field. A document without a doc type, or whose doc type has no class, is never disposed of. The page lists doc types with no class.

## Reviewing what is due

Open **Admin** › **Disposition** once a month. **Eligible for disposition** lists, client by client, the documents whose retention has ended, each with its doc type, class, tax year and the date it became eligible. Documents under a [legal hold](help:trash) are left out. Nothing is destroyed on its own.

Tick documents (or use **Select all** and **Select none**), then:

- **Approve N**: asks "Approve N documents for destruction?" with the date. Press **Yes, approve N**. They are destroyed after 30 days, unless a legal hold covers them by then.
- **Defer N**: takes them off the report for 12 months.

## The destruction run

**Waiting for destruction** lists what has been approved, with the date each can be destroyed and who approved it. Changed your mind? **Cancel** beside a document takes back its approval: it is not destroyed and returns to the list above for a new decision (recorded in the audit trail). Once some are due, type the admin password in **Password** and press **Destroy N due now**.

For each document, every copy of its content is overwritten and deleted: the file, its versions (when no other document has the same content), copies in the trash and Intake's working folders, and its OCR text. The catalog keeps a row marked as disposed. A certificate of destruction (PDF) is filed in the cabinet's `_Vibe\disposition` folder.

The **Runs** list shows each run with its date and count; **Open certificate** opens its PDF.

> Destruction cannot be undone. Without an admin password set, the page says "Set the admin password (Admin › Admin password) to destroy approved documents."

> Approvals are kept in this PC's catalog. Review and destroy from one PC only. Legal holds are read again from the cabinet just before anything is destroyed, so a hold placed on any PC still stops it.

## Legal holds

A legal hold freezes a client or chosen documents: nothing it covers is disposed of or emptied from the trash until it is released. See [trash and legal holds](help:trash) for placing and releasing holds.
