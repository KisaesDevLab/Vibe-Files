# When the Portal inbox loses a file

How the portal keeps what clients upload until Intake has filed it, and how to get back an upload missing from the Portal inbox.

## Why it is needed

`_Portal inbox` is a work queue on the cabinet share; it is not sent to B2. The portal is its safety net: it keeps each upload until Intake has filed it, then for the firm's keep period (30 days unless changed in the portal's Settings). With every fetch, Vibe Files reads the audit trail and tells the portal which uploads Intake filed, whichever PC filed them (moved, combined or split out of the inbox). An Undo that puts a file back keeps it unfiled.

## What is not filed yet

**Admin › Portal › Fetched, not filed yet** lists the rest:

- **in the Portal inbox**: waiting for Intake.
- **not in the Portal inbox, and Intake has not filed it** (moved or deleted by hand, or the inbox was lost): press **Fetch again** to put it back. It is refused if the file is still there or Intake filed it.
- **this cabinet has no record of it** (the list of received files was lost with the share): **Fetch again** too. Check first that it was not filed by hand.
- **Dismiss…**: for an upload not to be filed (junk, a duplicate). The portal deletes its copy after the keep period.

Browse says when uploads are missing from the inbox, have waited there over 7 days, or are within 14 days of the portal clearing them: it clears an upload never filed 180 days after it was fetched. This needs Vibe Portal 0.3.9 or later; an older portal is filed when fetched, as before, and Admin › Portal says to update it.

