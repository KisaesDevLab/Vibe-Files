# Text recognition (OCR)

How Vibe Files reads the words on scanned pages so you can search them, how to set up Tesseract on each PC, and why a file sometimes waits before its text is written into it.

## What it does

Every PDF in the cabinet is checked page by page. A page that already carries its own text (a PDF made by a program) is used as it is. A page with little or no text, such as a scan or a phone photo turned into a PDF, is read by Tesseract, a free text recognition program, at 300 dpi, in English.

The words are saved in this PC's catalog as soon as each page is read, so the document can be found on the [Search](help:search) tab right away. Documents filed while Vibe Files is running are read before the older backlog.

## Every document, or new filings only

A large cabinet (hundreds of thousands of files) would keep the PCs busy reading old scans for weeks. **Admin** › **Devices** › **Text recognition** sets what this catalog reads:

- **Read every PDF document**: documents already in the cabinet are read too, a few at a time, with new filings first.
- **Read only documents filed from now on**: only documents Vibe Files files from then on (Intake, scans, page tools) are read. Older files, and files put into the client folders outside Vibe Files, can still be found by file name and details, just not by the words on their pages.

Switching to new filings only takes the waiting backlog off the queue; pages already read stay searchable. A [folder import](help:folder-import) of more than 50,000 files chooses new filings only for you unless you tick **Read the text of the imported PDFs**.

## The text layer in the file

Once every page of a document is read, Vibe Files writes the words into the PDF itself as an invisible text layer. The page looks the same, but now the text can be selected and searched in any PDF program, in Explorer and in email. The file before the change is kept as a version, so nothing is lost (see [documents](help:documents)).

> A file that Intake has just filed is held: its text layer is not written while **Undo last batch** can still undo that batch, because Undo refuses a filed file whose size changed. The file is written once a newer batch is filed or the batch is undone. Searching already works meanwhile. See [undo](help:undo).

## Setting up Tesseract on a PC

Tesseract is installed separately on each PC that should read scans. Vibe Files looks for `tesseract.exe` in this order:

1. The path set in `config.toml` (in the data folder beside `vibe-files.exe`), section `[ocr]`, key `tesseract`.
2. A `tesseract` folder beside `vibe-files.exe`.
3. `C:\Program Files\Tesseract-OCR\tesseract.exe`, where the standard installer puts it.
4. Any folder on the Windows `PATH`.

Most offices only need to run the Tesseract installer with its default folder. To stop recognition on one PC, set `paused = true` in the same `[ocr]` section and restart Vibe Files.

## The footer

The status line at the bottom of the window says how recognition is going:

- **OCR: 12 documents to read**: working through the queue.
- **Search index up to date**: everything is read. If some files could not be read, it adds how many.
- **OCR waiting: … documents, no OCR engine**: Tesseract was not found on this PC. Install it, then restart Vibe Files.
- **OCR paused on this PC**: recognition is turned off here.
- **OCR off:** followed by a reason: recognition could not start, for example because `pdfium.dll` is missing beside the exe.

The queue is kept in the catalog, so closing Vibe Files part way through is safe: it picks up at the first page without text the next time.

> Recognition is never perfect. Faint, handwritten or skewed pages may give few words. A clean rescan usually helps.
