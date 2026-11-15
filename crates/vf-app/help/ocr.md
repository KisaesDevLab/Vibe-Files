# Reading scanned forms (Tesseract)

What Vibe Files reads from a scanned page, what it no longer reads, and how to set up Tesseract on a PC.

## What it does

When a scan or a phone photo reaches Intake, Vibe Files reads the page to recognise the form (a W-2, a 1099, a K-1, a 1098) and to suggest the client, the doc type and the year. A page that already carries its own text (a PDF made by a program) is used as it is. A scanned page is read by Tesseract, a free text recognition program.

That is the only reading it does. See [Intake](help:intake) and [scanning](help:scanning).

## What it does not do

Vibe Files does not read the documents already in the cabinet, does not keep the words of any page, and does not write a text layer into a PDF. [Find documents](help:search) in Browse finds documents by file name, folder and details.

Earlier versions read every PDF so the words on its pages could be searched. On a large cabinet that kept the PCs and the network busy for weeks, so it was taken out. A PDF that got a text layer then keeps it.

## Setting up Tesseract on a PC

Tesseract is installed separately on each PC that files scans. Vibe Files looks for `tesseract.exe` in this order:

1. The path set in `config.toml` (in the data folder), section `[ocr]`, key `tesseract`.
2. A `tesseract` folder beside `vibe-files.exe`.
3. `C:\Program Files\Tesseract-OCR\tesseract.exe`, where the standard installer puts it.
4. Any folder on the Windows `PATH`.

Most offices only need to run the Tesseract installer with its default folder.

> Without Tesseract, scans are still filed: Intake may not be able to suggest the form, and you choose the client, doc type and year yourself.

> Recognition is never perfect. Faint, handwritten or skewed pages may not be recognised. A clean rescan usually helps.
