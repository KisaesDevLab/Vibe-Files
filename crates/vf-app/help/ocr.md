# Reading text (OCR)

How to read the text of a scanned PDF in the Viewer, what Intake reads on its own, and how to set up Tesseract on a PC.

## A scan has no text

A PDF made by a program carries its text. A scanned page is a picture: nothing on it can be found, copied or picked for the 10-key tape until its text is read. Vibe Files reads it with Tesseract, a free text recognition program (OCR).

## Reading a document in the Viewer

Open the PDF in the Viewer. With no unsaved changes, click **OCR** on the toolbar and choose:

- **This page** reads the page shown. It takes a few seconds.
- **Whole document** reads every page that has no text. A long scan takes a few seconds a page; a line shows **Reading page 3 of 40…** and the rest of Vibe Files can be used meanwhile. **Cancel** stops after the page being read and leaves the document as it was.

When it is done the note reads **Text added to 12 pages; the previous version is kept.** The pages look exactly the same. The text is saved into the PDF as a new version, so it is there for everyone and in any PDF program, and the scan as it was is kept in the document's history.

Pages that already have text are left alone. If **This page** finds text on the page (a stamp or a header over a scan), it asks **Read it anyway?**

The 10-key tape offers the same thing: on a scanned page, **Pick from page** says the page has no text and shows **OCR this page**. See [10-key tape](help:ten-key).

A digitally signed PDF is not read: adding text would make its signature invalid.

## What Intake reads on its own

When a scan or a phone photo reaches Intake, Vibe Files reads the first page to recognise the form (a W-2, a 1099, a K-1, a 1098) and to suggest the client, the doc type and the year. Nothing is written into the file. See [Intake](help:intake) and [scanning](help:scanning).

## What it does not do

Vibe Files does not read the documents in the cabinet on its own, and does not keep the words of any page. [Find documents](help:search) in Browse finds documents by file name, folder and details, not by the words on their pages.

Earlier versions read every PDF unasked. On a large cabinet that kept the PCs and the network busy for weeks, so it was taken out. Now a document is read only when someone asks for it in the Viewer.

## Setting up Tesseract on a PC

Tesseract comes in the `tesseract` folder beside `vibe-files.exe`. Vibe Files looks for `tesseract.exe` in this order:

1. The path set in `config.toml` (in the data folder), section `[ocr]`, key `tesseract`.
2. A `tesseract` folder beside `vibe-files.exe`.
3. `C:\Program Files\Tesseract-OCR\tesseract.exe`, where the standard installer puts it.
4. Any folder on the Windows `PATH`.

> Without Tesseract, **OCR** says it was not found and nothing is changed. Scans are still filed: Intake may not be able to suggest the form, and you choose the client, doc type and year yourself.

> Recognition is never perfect. Faint, handwritten or skewed pages may be read wrongly or not at all. Check a picked amount against the page.
