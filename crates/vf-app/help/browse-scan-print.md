# Scanning into a folder and printing from Browse

The **Scan** and **Print…** buttons above the Browse grid: scanning straight into a client's folder, and printing the selected documents.

## Scan from Browse

**Scan** uses this PC's scanner and scan profile, the ones chosen on the Intake tab. Where the pages go depends on what Browse is showing:

- **A folder inside a client is shown** (for example `Baker Farms (1004) / Income Tax / 2025` in **Folders**, or `2025` › `Income Tax` in **By year**): the scan is filed straight into that folder as one document. A note says **Scanning on** followed by the scanner name and **It is filed into** the folder **when done.**
- **Anything else is shown** (the whole cabinet, a client's top level, or a view such as **Recent**): the pages go to this PC's Scans folder and then to Intake, the same as Intake's own Scan. Barcode cover sheets in the stack split and file them. See [Scanning](help:scanning) and [Cover sheets](help:cover-sheets).

### Scanning into a folder, step by step

1. In the tree, pick the client's folder, such as `Income Tax` › `2025`.
2. Load the pages and click **Scan**.
3. The scanner driver's own window opens (see below). Make your choices there and start the scan.
4. When it finishes, you see **Scanned into** and the path, followed by **Set its doc type in the details.**
5. Select the new document and set its **DOC TYPE**. See [Document details](help:documents).

The new document gets its tax year from the folder when the folder is a year (`2025`), and its source is set to **Client**. Every page that comes off the scanner, both sides of duplex pages included, ends up in the one PDF.

> If the scan cannot be filed, the pages are not lost: the message says **Scanned, but not filed** and tells you where the PDF went (this PC's Scans folder for Intake, or where it was kept).

### The driver's own window

By default the scanner driver's own window opens, so its settings and profiles (such as PaperStream IP) are used. To scan with the Vibe Files profile instead, turn off **Use the driver's own window** on the Intake tab. The setting is kept on this PC.

### When Scan does not start

- **No scanner found on this PC** — plug the scanner in, then click **Look again** on the Intake tab.
- **A scan is running.** — wait for the current one to finish.
- **Choose a Scans folder first** — needed only when the scan goes to Intake; set it on the Intake tab.

## Print from Browse

1. Select the documents in the grid.
2. Click **Print…**. It is greyed out until something is selected.
3. The Windows print dialog opens. Choose the printer and number of copies.

Each document is printed as its own print job. PDFs are printed by Vibe Files; other files are handed to their own program to print. While printing, the bar shows **Printing to** and the printer name; when done, **Sent 3 documents to** the printer.

If any selected document seems to hold an SSN or EIN, Vibe Files first asks **Print documents with taxpayer IDs?** Click **Print anyway** or **Cancel**. Every print is recorded in the audit trail. See [Audit trail](help:audit).

To print some pages only, open the PDF in the Viewer and print from there. See [Printing](help:printing).

## Also on this toolbar

**Print cover sheet** makes a barcode sheet that, scanned in front of a document, files it to a client. See [Cover sheets](help:cover-sheets).
