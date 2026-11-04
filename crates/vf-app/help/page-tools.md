# Page tools

How to rotate, delete, reorder, extract and split pages, flatten, make PDF/A, compress, and redact taxpayer IDs for good, all from the Viewer.

## The Pages window

In the [Viewer](help:viewer), click **Pages…**. Type the pages to work on in **Pages**, like `1-3, 5`; it starts with the page you are on.

- **Rotate left** and **Rotate right** turn those pages in the file.
- **Delete pages** removes them.
- **Reorder**: list every page in **New order**, for example `3, 1, 2`, then click **Reorder**.
- **Insert blank page**: type the page it goes after in **Blank page after page** (it starts as the page you are on; `0` puts it before the first page), then click **Insert blank page**. The blank page has the size and turn of the page beside it.

Each of these makes a new version of the file. The previous version is kept and can be restored from the document's details in [Browse](help:documents).

## Extract and split

Under **Extract the pages to a new document**:

- **Extract** copies the pages in **Pages** into one new document.
- **Split into documents** makes one new document for each range in **Pages**: `1-2, 3-5` gives two.

The document you are viewing is not changed. The new ones keep its doc type. Leave **Client** on **This document's client** and **Year** empty to put them beside the original; choose another client or type a year (or `Permanent`) to put them in that client's `Other` folder, in the year's folder under it (`Other` itself for `Permanent`).

## Flatten, PDF/A and Compress

- **Flatten** burns annotations and form fields into the pages, so they can no longer be moved or deleted.
- **PDF/A** makes the file PDF/A-2b for long-term archiving. Pages whose fonts are not embedded become images with a searchable text layer.
- **Compress** brings scanned images down to 200 dpi. If the file would not get smaller, it is left alone.

Each makes a new version and keeps the current one. These buttons, and **Pages…**, are greyed out while annotations are unsaved: save or discard them first.

## Combining PDFs

To join several PDFs into one, select them in Browse and click **Combine**. See [browse](help:browse).

## Redaction

Redaction removes text for good. It is not a black box laid over words that are still there.

1. On the panel at the right, open **Redact**.
2. Mark what to remove. **Draw box** lets you drag a box over any area; **Select text** lets you drag over words.
3. Or click **Auto-find patterns** to find SSNs and EINs. Turn on **+ Account numbers** or **+ Dates of birth** first to look for those as well. Each find is listed with its page and a masked value; untick any you want to keep. An EIN is often the payer's and is usually kept.
4. Click **Apply … & verify** (it shows how many redactions).

Vibe Files removes the text and images under the boxes, strips the file's hidden details, and saves a new document named `… (redacted).pdf` beside the original. Then it reads the new file's text again. If anything that should be gone is still there, nothing is saved and you see why. The original is not changed. The counts of what was removed go to the [audit trail](help:audit).

**Clear drawn boxes** removes the boxes you drew; ticked auto-find results stay.

> Send clients and third parties the redacted copy, not the original. Check its name ends with `(redacted)` before you attach it.
