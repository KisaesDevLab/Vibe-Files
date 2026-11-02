# SentinelV1 — `_Vibe/client.json`

**Source of truth:** Vibe Filer Desktop `src/sentinel.rs` at `ea4e726` (vendored as
`crates/filer-intake/src/sentinel.rs`). This document describes that code; if they disagree, the
code wins and this file is fixed. Practice Manager is not consulted (owner 2026-11-02).

## Location

`<cabinet>/<client folder>/_Vibe/client.json`. The `_Vibe` folder is a normal visible folder (no
Hidden attribute). One sentinel binds one top-level client folder to one client.

## Shape

```json
{
  "version": 1,
  "client_id": "0192f4a1-7c3e-7d10-9a2b-3c4d5e6f7a8b",
  "firm_id": "0192f4a1-7c3e-7d10-9a2b-000000000001",
  "tax_software_id": "1042",
  "display_name_at_creation": "Anderson Farms LLC",
  "created_at": "2026-11-02T16:20:00.000000Z",
  "created_by": null
}
```

| Field | Type | Meaning |
|---|---|---|
| `version` | integer | Always `1`. Any other value is read as **unparseable**. |
| `client_id` | UUID | The client's stable uuid (`Client::uuid`), assigned once per CSV ID and kept across client-list refreshes. Vibe Files' `client.id` in the catalog is this value. |
| `firm_id` | UUID | Generated once per installation into `config.json` (`Settings::firm_id`). Vibe Files stores it in `cabinet.json` / `cabinet.firm_id`. |
| `tax_software_id` | string | The client's ID from the CSV / tax software (e.g. UltraTax client ID). Vibe Files' `client.ext_id`. |
| `display_name_at_creation` | string | Client name when the sentinel was written. Historical; never updated. |
| `created_at` | RFC 3339 UTC | When the sentinel was written. |
| `created_by` | string or null | Always `null` from Filer. |

Serialized with `serde_json::to_vec_pretty`, fields in the order above. Readers must accept any
field order and must reject unknown `version`s.

## Rules

- **Write-once.** Written only when a folder is created or bound (`write_sentinel`): opened with
  `create_new`, `sync_all`, refused if one exists. `client_id` is never rewritten in place.
- **Inside the root only.** The folder must resolve inside the cabinet root (`ensure_inside`).
- **Nil ids refused.** A client or firm without a uuid cannot get a sentinel.
- **Re-binding** never deletes the old sentinel: it is renamed to
  `_Vibe/client.<kind>-<YYYYMMDD-HHMMSS>[-n].json` (`set_aside_sentinel`, kind = `orphaned` or
  `duplicate`) and a new one is written. Both steps go to the op log (→ audit in Vibe Files).

## Folder states from a root scan (`scan_root`)

| State | Meaning |
|---|---|
| `bound` | Sentinel present; its client is in the master list. |
| `unbound_named` | No sentinel; the client ID in the folder name is in the master list. |
| `unbound_fuzzy` | No sentinel; folder name is close to a client name (score kept). |
| `orphan` | Sentinel names a client not in the master list. |
| `unparseable` | Sentinel can't be read or isn't SentinelV1. |
| `duplicate` | Two or more folders carry sentinels for the same client. Routing and copies for that client pause until resolved. |
| `unmatched` | No sentinel, no matching client. |
| `ignored` | On the persisted ignore list. |

Names starting with `_`, `.` or `~`, and hidden/system folders, are skipped by the scan.

## How Vibe Files uses it

- `vibe-files rebuild` reads sentinels to rebuild `client` rows (`id = client_id`,
  `ext_id = tax_software_id`, `folder_rel`, `sentinel_hash` = SHA-256 of the file bytes).
- B2 sync keys a client by `client_id`, so renaming the folder is a manifest change only
  (`docs/BUCKET_LAYOUT.md`).
- A test in `vf-fs` (`sentinel_doc_matches_filer`) checks this file's example against a sentinel
  written by the vendored code.
