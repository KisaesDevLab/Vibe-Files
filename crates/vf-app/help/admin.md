# The Admin tab

A tour of every Admin page, in the order the left-hand list shows them, and how the admin password locks the tab.

## Locked by the admin password

When the firm has set an admin password, the whole Admin tab waits behind **Admin is locked** until someone types the password and presses **Unlock**. It then stays unlocked on this PC until Vibe Files closes, or until you press **Lock Admin** at the top of the list. The same unlock opens the Sync tab's settings.

With no password set, the top of the list shows **No admin password: set one**, and anyone can change Admin and Sync settings. See [the admin password](help:admin-password).

## FIRM

- **Devices**: the PCs that have opened this cabinet. See [devices](help:devices).
- **Doc types**: the list of document kinds, with their default folder and retention. The count is in the name. See [doc types](help:doc-types).
- **Folder templates**: the folders a new client gets. See [folder templates](help:folder-templates).
- **Folder import**: adopts folders already in the cabinet and binds them to clients. See [Importing an existing folder tree](help:folder-import).

## INTAKE

- **Print capture & parsers**: the folder a PDF printer saves into, and the file-name profiles (UltraTax CS is included) that file printed returns to the right client. See [print capture](help:print-capture).

## COMPLIANCE

- **Audit trail**: every change, hash-chained, with filters, exports and **Verify now**. See [the audit trail](help:audit).
- **Retention classes**: how long each kind of document is kept.
- **Disposition**: documents whose retention has ended, to approve or defer, and the destruction runs with their certificates. See [retention and destruction](help:retention).
- **Legal holds**: every hold in the cabinet, with **Release**. See [trash and legal holds](help:trash).
- **Safeguards appendix**: one PDF for the firm's written information security plan (inventory, encryption, workstations, retention and holds, integrity, backup, disposition runs). Pick **Period from** and press **Export appendix…**.

## SECURITY

- **Admin password**: set, change or remove it.
- **Integrity sweep**: re-checks the client folders against the catalog. See [rebuild, sweep and checks](help:maintenance).
- **Trash**: deleted documents, to restore or empty. See [trash and legal holds](help:trash).

Greyed entries in the list are not in this version.

> Most Admin settings are kept in the cabinet's `_Vibe` folder, so every PC that opens the cabinet uses them; the print capture folder is set per PC. Intake on other PCs offers **Reload Intake** when its settings change.
