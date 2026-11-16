# Where the catalog is kept

How an admin chooses the folder each computer keeps its catalog in. The catalog is Vibe Files' index of the cabinet. It holds no documents and can always be made again from the cabinet.

## The default

A computer's shared catalog is on its own disk, in `ProgramData\Vibe Files\catalogs`. This is the fastest and safest place. **Admin** › **Devices** › **This PC's catalog** says which place is in use.

## Choosing another place

Add one line to `firm.toml` beside `vibe-files.exe`:

- `catalogs = 'exe'` keeps it beside the program: `catalogs\<computer name>\` in the program's folder, one folder per computer.
- `catalogs = 'D:\VibeFiles'` keeps it in that folder, again one folder per computer.
- `catalogs = 'programdata'`, or no line, is the default.

The change takes effect the next time Vibe Files starts on each computer. The catalog a computer already has is copied to the new place the first time, so the cabinet is not indexed again. Removing the line moves it back the same way.

Every computer has a folder of its own because a catalog belongs to one computer. Two computers never open the same one.

> A place on a network share is slower than the computer's own disk, and while one person's change is being saved the others wait a moment. A catalog there can also be damaged if the connection drops during a save. If that happens, delete that computer's folder and open Vibe Files: the catalog is made again from the cabinet.

## Which computers this applies to

It applies to a computer's shared catalog: on a Remote Desktop server, for a cabinet in OneDrive or Dropbox, or after **Share this catalog with everyone on this PC**. An office PC that has not shared its catalog keeps it in its own data folder. See [Several PCs](help:several-pcs).
