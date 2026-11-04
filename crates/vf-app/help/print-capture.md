# Print capture and UltraTax

How PDFs printed from UltraTax CS (or any program) reach Intake already named, typed and dated: the watched printer folder, the UltraTax file name layout, and the list of UltraTax documents with where each one is filed.

## The print capture folder

Open Admin › **Print capture & parsers**. Next to **Print capture folder (this PC)**, press **Choose…** and pick a folder on this PC, outside the cabinet and outside Intake's inboxes. It must not be the Scans folder. **Stop capturing** turns it off. Each PC sets its own folder; the settings below are shared by every PC.

Any PDF saved into that folder goes to Intake as the firm's document. If its name can be read, it is named for the client and given its doc type and year. If not, it still goes to Intake under its own name.

## Setting up UltraTax CS

In UltraTax CS, open Setup › Office Configuration › Print Options › PDF File Options, and save PDFs to the print capture folder. Then match the profile here to what you set there.

### Options

The **Options** tab matches UltraTax's Options tab:

- **File Name Variables** and **Variable Position**: for each part of the name (**Client ID**, **Client Name**, **Tax Year**, **Document Name** and so on), the position UltraTax puts it in, or **None**.
- **K1 Recipient Name**: **After Document Name** if UltraTax adds the partner's name.
- **Print separate PDF files for each federal and state return**, with **Include Postal Code**.

**File Name Example** shows a name built from your choices; it should look like the names UltraTax prints. **Restore Default** puts back the office's layout.

### Document Names

The **Document Names** tab lists UltraTax's documents, grouped as government copy, client copy, preparer copy, organizer and other. For each row:

- **Custom Name**: the name set in UltraTax, if any. Prints with either name are read.
- **Doc type**: the doc type the document gets. `K-1 (by entity type)` picks the K-1 doc type from the return's form.
- **Filing**: **File**, or **Don't file** to keep the print out of the cabinet (it stays in the folder's `_processed` folder).
- **Folder**: where it goes below the client folder. Empty means the doc type's own folder.
- **File name**: the name it is filed under. Empty means the choice under **File names**: as printed, or the short name (`2025 1065 Tax_Return.pdf`).
- **Partner copy**: for K-1 rows, also file a copy to the partner's folder.

As set up for the office, government copies go to `Income Tax/{Year}/Government Copy`, preparer copies to `Workpapers & Support/{Year}`, and client copies to their doc type's folder. Only the client-copy K-1 package is copied to partners.

**Documents not in UltraTax's list** holds extra rules for other document names.

## Trying a name and saving

Under **Try a file name**, type a name such as `Baker Farms_2025_1065_Tax_Return_1004.pdf` and press **Try**. It shows the client ID, year, doc type, the name Intake will get (**Reaches Intake as**) and where it will be filed (**Filed as**).

Press **Save** to keep your changes, or **Discard**. Saving also updates Intake's rules (see [rules](help:intake-rules)). Intake then shows a banner: press **Reload Intake** there to use the new folders.

> A print set to **Don't file** shows in the Intake strip as not filed and kept in `_processed`.
