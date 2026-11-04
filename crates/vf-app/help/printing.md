# Printing

How to print a document from the Viewer, or several at once from Browse, what the SSN and EIN warning means, and how each print is recorded.

## Print from the Viewer

1. Open the document in the [Viewer](help:viewer).
2. Click **Print…** in the toolbar.
3. The Windows print dialog opens. Choose the printer and the number of copies. Under the page range you can print all pages, the current page (the one you are looking at), or pages like `1-3, 5`.
4. Click Print.

Saved annotations, stamps and pasted tapes print as they show on screen. Annotations that are still unsaved do not print: save them first (see [annotations](help:annotations)).

## Print several documents from Browse

1. In [Browse](help:browse), select the documents (Ctrl+click or Shift+click to pick several).
2. Click **Print…** in the toolbar above the list.
3. Choose the printer and copies in the Windows print dialog. From Browse, every document prints in full: to print only some pages, open the document in the Viewer.

Each document goes to the printer as its own print job. If one fails, the others still print, and the message afterwards names the one that did not print and why. See also [scanning and printing from Browse](help:browse-scan-print).

While printing, the toolbar shows **Printing to** and the printer's name. When it is done you see, for example, "Sent 3 documents to Front Office Printer."

## Files that are not PDFs

Vibe Files prints PDFs itself, so it does not depend on which PDF program is installed. Other files, such as Word or Excel documents, are handed to their own program to print, the same as choosing Print when you right-click the file in Explorer. That program uses its own settings, usually the Windows default printer, so the printer and copies chosen in the dialog may not apply. If no program on the PC can print a file, the message says so.

## Taxpayer ID warning

Before the print dialog opens, Vibe Files checks the text of the documents for anything that looks like a Social Security number or an EIN. If it finds one, it asks **Print documents with taxpayer IDs?** and says how many of the documents have one. Printed copies leave the cabinet's protections.

- **Print anyway** goes on to the print dialog.
- **Cancel** prints nothing.

> The check uses the text Vibe Files has read. A scan that text recognition has not read yet is not checked (see [text recognition](help:ocr)).

## The audit record

Every document sent to the printer gets one entry in the [audit trail](help:audit): which document, how many pages, how many copies and which printer. No page content is stored. A print of several documents from Browse is recorded together, at the same time. If the audit entry could not be written, the message after printing says so.
