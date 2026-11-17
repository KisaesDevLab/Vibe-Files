# When Intake asks first

The cases where Intake shows a row for you to decide, where it could have guessed.

Intake files a document on its own when the file name says clearly whose it is and for which year. In the cases below the name leaves room for doubt, so the row waits in **Preview** with a suggestion or a status, and nothing is filed until you choose.

## The name has an ID no client has

A file named `Miller, Robert_9999_2025 W-2.pdf` carries the ID 9999. If no client has that ID, the file is for a client who is not in the client list yet, however well the name matches someone who is. The client of that name is offered as a suggestion; the row says **Needs client**. Click the suggestion to take it, or use **New client…** for a new one.

A year or a form number in that place (`Smith, John_2025_W-2.pdf`) is not an ID: only something shaped like your own client IDs counts.

Two clients of the same name are offered as well, not picked between.

A name with no ID of its own that is exactly a client's name is still matched without a click, with a `fuzzy` pill.

## A word that only happens to equal a client's ID

`1099-R Fidelity.pdf` is not for a client whose ID is 1099. An ID found as a word in a name is taken as the client's only when:

- it stands as a part of its own between underscores, first, second or last: `1099_2025 W-2.pdf`, `Smith, John_1099_2025 W-2.pdf`, or `Smith, John_2025_Return_1099.pdf`; or
- the client's name is in the file name too.

Otherwise the client is offered as a suggestion.

Prints from UltraTax and files that came through print capture are not affected: their ID is read from its place in the name.

## Two different years in the name

`Smith 2020 Family Trust 2025 1041.pdf` does not say which year is the tax year. The row says **Needs year**. Type the year on the row, or set the batch's year above the list, which settles every such row.

A name with one year, or the same year twice, is read as before. A year in the client's name in front of the ID (`Smith 2020 Family Trust_1001_2025 1041.pdf`) is not counted.

## A folder with no client file

When a file is filed for a client who has no bound folder, Intake looks for a folder that ends in the client's ID. It binds that folder to the client only if the folder's name has the client's name in it (either way round: `John Smith (1001)` for Smith, John) and it is the only such folder. A folder that merely ends in the ID (`Scans_2024` and a client 2024) is left alone, and the client gets a folder of its own. Bind a folder by hand in **Setup**.
