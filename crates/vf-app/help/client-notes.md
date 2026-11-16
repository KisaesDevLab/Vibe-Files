# Client notes

How to read and write a client's notes in Browse. They are the same notes FileCenter shows for the client's folder.

## Where they are

Pick a client in the tree on the left, with no document selected. The details pane on the right ends with **NOTES** and a box with the client's notes.

The notes show when the client itself is selected. In one of the client's folders, or with a document selected, the pane has no notes.

## Where they are kept

The notes are kept in a file named `!fcinfo.ini` in the client's top folder. FileCenter keeps its notes for a folder in that file, in the part headed `[Memo]`. Vibe Files reads and writes that part only. Everything else in the file is left exactly as it was, so FileCenter goes on reading it.

`!fcinfo.ini` is not a document. It is not listed in Browse's **Documents** list and is not counted. **Explorer** view still lists it, because that view shows the folder as it is on disk. Leave the file where it is.

## Backup to B2

With B2 sync set up, a client's notes files are backed up to B2 with the next push after they change: at once for notes saved in Vibe Files, and after the next integrity sweep (weekly, or **Admin** › **Integrity sweep**) for notes changed in FileCenter. The files in the client's sub-folders go too. A restore from B2 puts each notes file back where the folder has none; notes that are there are never replaced. An ordinary pull on another PC does not bring notes over.

## Changing the notes

1. Type in the box. It grows with the text. **Not saved** shows beside the buttons.
2. Click **Save notes**.

**Revert** puts back the notes as they are in the file. **Reload** reads the file again, for example after someone changed the notes in FileCenter.

If you go to another client before saving, what you typed is kept and shown again when you come back, until you save or revert it, or close Vibe Files.

## A client with no notes yet

The box is empty and the button reads **Create notes**. Type the notes and click it: Vibe Files makes `!fcinfo.ini` in the client's folder, in the form FileCenter makes it.

## When someone else changed the notes

If the file was changed after Vibe Files showed it (in FileCenter, or on another PC), **Save notes** does not write over that change. The pane says so and shows what the file has now, with two choices:

- **Take theirs** shows the file's notes and drops what you typed.
- **Save mine anyway** writes what you typed over the notes in the file.

> FileCenter does not check in the same way. If FileCenter has the client's notes open and saves them after you saved here, its older text replaces yours. Close the notes in FileCenter, or reload them there, before you edit them here.

## What is recorded

The audit trail records that a client's notes were changed, when and on which PC. It does not record what the notes say.
