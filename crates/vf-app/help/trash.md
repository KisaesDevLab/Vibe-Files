# Trash and legal holds

What happens when you delete a document, how to get it back, how the trash is emptied, and how legal holds keep documents from being emptied or destroyed.

## Deleting a document

In Browse, right-click a document and choose **Move to trash**, or select several and press **Move N to trash**. For more than one you are asked first: press **Yes, move N to trash**.

The files move to the cabinet's `_Vibe\trash` folder; nothing is deleted yet. **Undo last** puts them straight back (see [undo](help:undo)). If sync is on, the other PCs move them to their own trash on their next pull.

## Restoring from the trash

1. Open **Admin** › **Trash**. Each deleted document shows **Was at**, **Deleted**, **Why** and **Size**.
2. Tick the documents you want back.
3. Press **Restore N**.

Each goes back to where it was. If another file now has that name, the restored one gets a ` (2)` after its name.

A file moved to the trash from Intake (right-click › **Move to trash**) is listed with where it was in the inbox. **Restore** puts it back in the inbox, where Intake picks it up again.

## Emptying the trash

Documents are emptied on their own once they have been in the trash for 90 days. This happens when the cabinet is opened.

To empty by hand, type the admin password in **Password** and press **Empty trash** (everything), or tick some and press **Empty N for good**. This deletes the files for good and cannot be undone.

> Emptying by hand needs an admin password to be set. Without one the page says "Set an admin password (Admin › Admin password) to empty the trash by hand." See [the admin password](help:admin-password).

## Legal holds

A legal hold freezes a client, or chosen documents, during an examination, a dispute or a request. While a hold is in place, nothing it covers is emptied from the trash (by hand or after 90 days) or destroyed by [disposition](help:retention). Held documents show "legal hold" in the trash list.

### Placing a hold

1. In Browse, choose a client in the tree, or select documents.
2. Press **Place legal hold on this client…** (or **on this document…**, **on these N documents…**).
3. Type the **Hold reason**, for example `IRS exam 2023`, and press **Place hold**.

Browse then shows a red "Legal hold:" line with the reason and date whenever that client or those documents are selected. Holds are kept in the cabinet (`_Vibe\legal_holds.json`), so every PC honors them.

### Releasing a hold

Press **Release hold** on that line in Browse, or **Release** beside it in **Admin** › **Legal holds**. You are asked "Release the legal hold …?"; type the admin password in **Password** if the firm has one, and press **Yes, release**. Everything the hold covered can then be emptied and disposed of again.
