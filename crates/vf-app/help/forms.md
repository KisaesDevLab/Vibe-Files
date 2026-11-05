# Filling in forms

How to fill in a PDF form in the Viewer, save it so it can still be changed, and save a flattened copy that can't be.

## Which PDFs have a form

A fillable PDF has fields: boxes to type in, check boxes, round option buttons and drop-down lists. Government forms downloaded as fillable PDFs and many organizers have them. In the [Viewer](help:viewer), the **Form** tab on the panel at the right shows how many fields the document has, for example **Form (12)**.

A scanned form is a picture of a page and has no fields. To write on it, use **Text** on the **Annotate** tab (see [annotations](help:annotations)).

## Filling it in

Open the **Form** tab. Every field is outlined in blue on the page:

- Click a box and type. **Tab** moves to the next field.
- Click a check box to tick it or clear it. Click an option button to choose it.
- Click a drop-down list on the page to pick from it.

The tab also lists every field by page, with its name, and you can fill them in there instead. Click a **Page** heading to go to that page. Greyed-out fields are read-only and can't be changed.

What you type counts as unsaved, like an annotation: the toolbar shows **N unsaved**, and Vibe Files asks before closing the document or the program. **Undo form changes** at the bottom of the panel clears everything typed since the last save.

## Saving

- **Save** writes what you typed into the file as a new version. The previous version is kept. The form stays fillable, here and in other PDF programs, so it can be changed later.
- **Save as…** writes a new document beside this one with what you typed in; this file is not changed.
- **Save flattened copy** on the **Form** tab writes a new document named `… (flattened).pdf` beside this one. Its fields, and any unsaved annotations, are burnt into the pages and can't be changed. Use it for the copy you send or sign. This document is not changed, and what you typed stays unsaved here until you **Save** it or discard it.

To lock the document itself once it is final, use **Flatten** on the toolbar (see [page tools](help:page-tools)).

The [audit trail](help:audit) records that a form was filled and how many fields changed. It never records what was typed.

Some government forms also hold a second, Adobe-only copy of their fields (XFA). Saving a filled form removes that copy, so Acrobat shows what you typed rather than the old values.

Some PDFs open without a password but are locked against changes (an "owner" password). Vibe Files fills them and saves the filled file without that lock, so it opens and can be changed anywhere. A PDF that needs a password to open can't be filled here: the **Form** tab says so. Fill it in the PDF program that has its password.

## Signed PDFs

Saving changes into a PDF that has a digital signature makes the signature invalid. **Save** on a signed PDF asks first: **Save as a new document** keeps the signed file as it is and puts your changes in a new one; **Save into the signed file** saves a new version anyway (the signed version stays in its history).

A list that allows several choices keeps one choice here.

> Characters outside the usual Western European ones (for example Greek or Chinese) are kept in the field but show as `?` on the page.
