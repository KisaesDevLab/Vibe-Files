# Doc types

The firm's list of document kinds (W-2, K-1, engagement letter and so on): what each field means, how to add, edit, order, archive and remove them.

## Where to find them

Open **Admin** › **Doc types**. The count in the name is how many the cabinet has. The list is kept in `_Vibe\doc_types.json` in the cabinet and in the catalog, so every PC uses the same list and a rebuild restores it.

The table shows **NAME**, **DEFAULT FOLDER**, **CLIENT-VISIBLE**, **DOCUMENTS** (how many documents use it) and **ORDER**.

## Adding or editing one

Press **Add doc type**, or click a name in the list to edit it. The fields:

- **Doc type name**: what people pick, for example `Crypto statement (1099-DA)`.
- **Default folder**: where a document of this type is filed inside the client folder, for example `Income Tax/{Year}`. Type it, or choose one from **Pick from template** (the folders of the [folder template](help:folder-templates)); **No default** clears it.
- **Client-visible**: marks documents of this type as ones the client may see.
- **Default on cover sheets**: **Print cover sheet** starts with this doc type. Only one doc type can be the default. See [cover sheets](help:cover-sheets).
- **Archived (hidden from pick lists)**: see below.
- **Retention**: the retention class that decides how long documents of this type are kept, or **None (never disposed)**. See [retention and destruction](help:retention).

Press **Add** for a new one, or **Save changes**. **Close** leaves without saving.

> A name that print capture's "K-1 (by entity type)" rows use cannot be renamed here; the message says so.

## Order

**Up** and **Down** in the **ORDER** column move a doc type in the list.

## Archiving

Tick **Archived (hidden from pick lists)** and press **Save changes** to stop offering a doc type when someone chooses one. Documents that already have it keep it, and its retention still applies. An archived doc type cannot be the cover sheet default.

Archived doc types are hidden from this table too. Tick **Show archived** (it shows how many) to see them, marked "(archived)". To bring one back, open it and untick the box.

## Removing

**Remove** deletes a doc type no document uses straight away. If documents use it, you are asked first: they keep it, shown as deleted, and this cannot be undone from Browse. Press **Yes, remove** or **Keep it**.

> Prefer archiving to removing a doc type that has been used: nothing is lost and you can bring it back.
