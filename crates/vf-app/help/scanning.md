# Scanning

How to scan paper into Vibe Files: picking the scanner and profile, what **Use the driver's own window** does, the Scans folder, and where the scanned pages end up.

## The Scans folder

Scans do not go straight into the cabinet. They land in this PC's Scans folder first, where Vibe Files splits them at [barcode cover sheets](help:cover-sheets) and hands the pieces to Intake.

At the top of the **Intake** tab, next to **Scans folder**, press **Choose…** and pick a folder. It must be outside the cabinet and must not be one of Intake's inboxes. The strip shows how many files are waiting.

You can also save files into the Scans folder yourself, for example photos from a phone. PDFs and pictures (`.jpg`, `.png`, `.tif`, `.bmp`, `.heic`) are taken; pictures are made into PDFs first.

## Picking the scanner

The **Scanner** row lists the scanners this PC can see, by their TWAIN or WIA driver. Pick yours from the list; Vibe Files remembers the choice for your Windows login on this PC. If yours is missing, plug it in and press **Look again**.

## Profiles

The profile box sets the paper source, sides, color and resolution:

- `Stack, both sides (gray 300)`
- `Stack, one side (gray 300)`
- `Stack, both sides (color 200)`
- `Flatbed (color 300)`

With a profile, pages with almost no ink are dropped as blank, and pages are straightened.

## Use the driver's own window

For a TWAIN scanner (such as one using PaperStream IP), the **Use the driver's own window** box is shown and is on by default. With it on:

- The scanner driver's window opens when you scan, and its own settings and profiles are used. The profile box is greyed out.
- Sides (one or both), color and resolution are set in the driver's window.
- Vibe Files keeps every page the driver sends, front and back. If you want blank pages removed, turn on the driver's own blank page skip. This way a faint back side is never thrown away by a second guess.

Turn the box off to scan with the profile chosen here instead.

## Scanning

1. Put the stack in the feeder (with cover sheets between documents, if you use them).
2. Press **Scan**.
3. When it is done, a line such as `Scanned Scan 2026-11-04 141502.pdf (14 pages · 2 blank dropped)` appears under the strip.

The PDF is saved in the Scans folder. Within a few seconds it is split at any cover or separator sheets and the pieces appear in Intake's queue, ready to file (see [Filing documents with Intake](help:intake)). The original scan is kept in the Scans folder's `_processed` folder. A file that could not be read goes to `_failed`, with a note saying why.

> A cover sheet's blank back is dropped in a both-sides scan even with the driver's window on, so you do not need to remove it.

## Scanning from Browse

The **Scan** button in Browse uses the same scanner and settings. With a client's folder open, the scan is filed straight into that folder as one document; see [Scan and print from Browse](help:browse-scan-print).
