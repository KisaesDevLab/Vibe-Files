# Checking Vibe Files with several users at once

For 0.1.45 and later. These are the cases a single PC cannot show. Each takes a few minutes.
Use two people, or one person with two logins. Write down what the screen said, word for
word, wherever a step does not go as described.

Where it says "the log", open **Admin › Devices › This computer's log › Show**, pick today,
and use **Copy**.

## A. Two logins on one session host

1. Login 1 opens the cabinet and starts something long: **Admin › Devices › Index the cabinet
   again…**, then closes and opens Vibe Files so the index starts.
2. While the footer says **Indexing…**, login 2 opens the cabinet on the same host.
   - Expected: it opens (it may say it is waiting for the catalog). Browse lists clients.
   - Not expected: any red line. In particular not "this is not a Vibe Files catalog",
     "Catalog integrity", "database is locked", or "is not in the catalog".
3. Login 2 opens a client, opens a document in the Viewer, and closes it.
4. Login 2 files one document from Intake (any inbox file) while login 1's index still runs.
   - Expected: it files. After the index, both logins see the document once.
5. When the index ends, login 1's footer says **Leaving a copy of the index for the other
   computers…**. Leave Vibe Files open until that line goes.

## B. The same login on two session hosts

1. Sign in to host 1 and host 2 with the same Windows login, and open the cabinet on both.
2. Put one PDF in the print capture folder (or scan one page).
   - Expected: it arrives in Intake once. Not twice, and not in the failed folder.
3. On host 1, change a client's B2 choice (client's details › B2 sync). On host 2, open the
   same client within a minute.
   - Expected: host 2 shows the new choice, or the old one for a few seconds; never "Not
     known just now" for longer than a moment.

## C. Two hosts, one finishing an index

1. Host 1: index the cabinet again (as in A.1) and wait for it to finish, and for the
   **Leaving a copy…** line to go.
2. Host 2: open the cabinet.
   - Expected: no full index. The footer may say **Reading changes made on other servers**
     for a short while.
   - If it does index: **Admin › Devices** on host 2 says when the index started and why.
     Copy that line and the log's lines containing `index`.

## D. Two people changing the same shared thing

1. Both open **Admin › Folder templates**. Person 1 renames a folder and saves. Person 2 then
   changes something and saves.
   - Expected: person 2 is told the template was changed on another computer and nothing was
     saved. After leaving the page and opening it again, person 1's change is there.
2. Both open **Sync › What goes to B2**. Same as above with a rule.
3. Person 1 runs **Admin › Similar clients › Find**, ticks one pair, and merges it. During the
   merge person 2 presses **Not the same** on another pair in the same list.
   - Expected: person 2's list is greyed while their own action runs; nothing freezes for
     more than a few seconds; both changes are there afterwards.

## E. The share going away for a moment

Only if it is easy to do safely (for example disconnecting and reconnecting the mapped drive
on one PC while nobody else works).

1. With the cabinet open and idle, disconnect the share for about ten seconds, then
   reconnect.
   - Expected: lines that say the cabinet cannot be reached or read just now. After
     reconnecting, Browse works again without closing Vibe Files.
   - Not expected: "is not a Vibe Files cabinet", an admin password that stopped working, a
     client that lost its documents, or a client list that came back empty.

## F. One scan on the real scanner

The scanner changes after 0.1.45 were tried only against a simulated scanner.

1. Scan a batch of about ten sheets, both sides, with one short sheet (a receipt or a half
   page) in the middle.
   - Expected: every side is in the PDF, in order. The short sheet is a short page, with no
     black band below it.
   - Not expected: a page missing without a line saying so, or a page cut off.
2. If the scanner jams or is stopped part way: the pages scanned before that are kept, and a
   line says why it stopped.

After 0.1.45 the scanner's driver runs in a helper program (a second `vibe-files.exe` in
Task Manager while a scan runs), so these need the real scanner too:

3. An ordinary scan with **Use the driver's own window** on: the driver's window opens in
   front, and the scan arrives in Intake as before.
   - If the scanner is not listed, or does not scan: tick **Run the driver inside Vibe
     Files**, try again, and send **Scanner details** (Copy).
4. Start a batch and press **Stop** in Vibe Files half way.
   - Expected: the scan ends within a few seconds; the pages scanned so far are the PDF; the
     line says the scan was stopped.
5. Start a batch and, half way, end the second `vibe-files.exe` (the smaller one) in Task
   Manager. This stands in for a driver that fails.
   - Expected: Vibe Files stays open; the pages before that are the PDF; the line says the
     scanner's driver stopped working, with a code.
6. One long batch: about 100 sheets, both sides, colour.
   - Expected: it finishes. Note Vibe Files' memory in Task Manager at its highest.

## What to send back

- For each lettered case: went as described, or what was on the screen instead.
- The log lines (Copy) from the login that saw anything unexpected.
- From **Admin › Devices**: the catalog line ("In use: …", the counts, and "The last index
  of this catalog started …").
