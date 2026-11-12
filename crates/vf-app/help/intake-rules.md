# Rules, the folder template and Intake Setup

How Intake decides where a file goes: the Setup screen (cabinet, inboxes, client list, ID pattern, folder template) and the Rules screen, including building a rule from a file in the queue.

## Setup

Open **Intake** › **Setup**. Change what you need, then press **Save** (or **Revert** to drop your changes).

> When the cabinet has an [admin password](help:admin-password), Setup is read-only until Admin is unlocked on this PC: its settings change every PC's filing.

- **Client root**: the cabinet. Vibe Files sets it to the cabinet you opened, so leave it as it is.
- **Inboxes**: the folders Intake watches. Press **Add inbox…** to add one, **Remove** to drop one. An inbox cannot be inside the cabinet, except in a folder at the top of the cabinet whose name starts with `_` (like the Portal inbox, `_Portal inbox`). Zips dropped in an inbox go to the **Import** screen.
- **Client master list**: the client CSV. **Choose CSV…** picks the file and maps its columns; **Refresh clients** reads it again; **Stop using the CSV** keeps the client list as it is and reads no CSV again (new clients are still added to the list); a CSV that was moved or deleted leaves the list as last imported, with a note; **Re-map columns…** changes which column is the ID and which the name, and under Optional which column holds each client's **Partner** (any header, such as Preparer; it becomes the `{Partner}` token and Browse's Partner filter). Clients added in Browse are in the same list (see [clients](help:clients)).
- **Filename parsing**: the **ID pattern** says what a client ID looks like in a file name. Vibe Files sets it to 4 or more capital letters or digits, so both `SMIT1001` and `1004` match, unless your firm changed it. The **Convention** line shows the shape Intake expects: `ClientName_ID_rest.pdf`, for example `Baker Farms_1004_2025 W-2.pdf`. **Suggest at** and **Auto-resolve at** set how close a name must be for a name match to be suggested, or to be used at once.
- **Folder template**: the folders a new client folder gets. Select a folder and use **Add child**, **Rename** or **Delete**; **Reset to default** brings back the standard tree. This is the same template as Admin › **Folder templates** (see [folder templates](help:folder-templates)); a change in one shows in the other. Existing client folders are not changed.

## Rules

Open **Intake** › **Rules**. Rules are checked from top to bottom against the part of the file name after the client ID; the first rule that matches wins. The last rule, **Catch-all rule**, takes whatever no other rule matched and cannot be deleted. Rules belong to a profile, chosen with **Profile** at the top of Intake.

Select a rule to edit it:

- **Name**: shown in the **Rule** column of Preview.
- **Pattern**: the words to look for, as a regular expression. **Whole words only** keeps `k-?1` from matching `K-10`. **Except when** is a pattern that stops the match.
- **Primary destination**: the folder below the client folder, for example `Income Tax/{Year}`.
- **Filename template**: the name the file gets, for example `{Year} {DocType}.{Ext}`.
- **Requires year**, **Also create next year's folder** and **Enabled**.

Use **Add rule** or **Duplicate** to add one, drag the handle to reorder, then **Save**. **Test a filename** shows which rule a name would get.

## Build a rule from a file

In Preview, right-click a row and choose **Build a rule from this file…**. The Rules screen opens with the file name in **Test a filename**. Under **Build a rule**, click the words that identify this kind of document, give the rule a name, pick a **Folder** and a **File name**, then press **Create rule** and **Save**. The new rule goes just above the catch-all rule.

> The rules that file print captures are kept in step from Admin › **Print capture & parsers**. Change them there, not on the Rules screen: see [print capture](help:print-capture).
