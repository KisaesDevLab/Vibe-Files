# Vibe Files bucket layout — v1

The contract `vf-sync` implements (P11–P12). Vibe Files' own layout; Practice Manager is not
consulted (owner 2026-11-02). Status: **draft until P11 writes the first real bucket**; after
that, any change bumps `layout_ver` and ships a migration.

## Goals

1. Restore a cabinet byte-for-byte from the bucket plus the recovery passphrase.
2. Client folders come back exactly as the Filer Desktop tree had them (`{Client} ({ID})/…` per
   the cabinet template), with the same SentinelV1 (`docs/SENTINEL_V1.md`).
3. The bucket listing reveals nothing about clients: no names, IDs, paths, or plaintext hashes in
   object keys. Paths live only inside the encrypted manifest.
4. Pushing one client touches only that client's manifest segment.

## Keys

Everything below lives under one prefix: `vf/<cabinet_id>/` (`cabinet_id` from `_Vibe/cabinet.json`).

| Key | Encrypted | Content |
|---|---|---|
| `cabinet.json` | no | `{layout_ver: 1, cabinet_id, firm_id, created, key_ids: ["k-01", …], kdf: {…argon2id params, salt}}`. No secrets. Lets a bare PC find the right key from the recovery passphrase. |
| `manifest/HEAD` | yes | `{rev, written_at, device_id, segments: {<client_id>: <segment_rev>}, cabinet_segment_rev}`. Written **last** on every push. |
| `manifest/clients/<client_id>/<segment_rev>` | yes | One client's segment (below). Immutable; a new rev is a new object. |
| `manifest/cabinet/<rev>` | yes | Cabinet-level rows: doc types, retention classes, users/roles (no secrets), devices. |
| `objects/<n[0..2]>/<n>` | yes | File content. `n = hex(HMAC-SHA256(naming_key, sha256(plaintext)))`. Same content → same object (dedupe across clients, versions, devices). |
| `thumbs/<n[0..2]>/<n>` | yes | Page-1 thumbnail of object `n` (P12 step 6). |
| `locks/<doc_id>` | yes | Check-out lock `{doc_id, user, device_id, since, lease_until}`. Deleted on check-in. |
| `devices/<device_id>` | yes | `{device_id, name, mode, last_seen, acked_rev}`. Tombstoned objects are removed only when every live device's `acked_rev` ≥ the tombstone's rev (and retention allows). |
| `catalog/<rev>` | yes | Catalog snapshot for remote mode (P12). Without FTS tables in v1. |
| `audit/<yyyy-mm>` | yes | Copy of `_Vibe/audit/<yyyy-mm>.jsonl` (append → new object version). |

`client_id` and `doc_id` are UUIDs (not names), and `n` is a keyed hash, so the listing carries no
client information.

**Never uploaded:** `_Vibe/keys/` (the passphrase-wrapped catalog key, P01), Filer's working folders
`_Vibe/tmp/`, `_Vibe/overwritten/`, `_Vibe/undo_zips/`, `_Vibe/split_originals/` (P02; their content
reaches the bucket as documents or versions, never as Filer scratch), and `_Vibe/locks/`
(LAN locks; the bucket has its own `locks/`). No server holds a key, wrapped or not.

## Client segment

```json
{
  "client_id": "0192f4a1-…",
  "segment_rev": 17,
  "folder_rel": "Anderson Farms LLC (1042)",
  "sentinel": { "…": "the SentinelV1 JSON, verbatim" },
  "documents": [
    {
      "doc_id": "0192f4a1-…",
      "rel_path": "Income Tax/2025/Anderson Farms LLC_1042_W-2_2025.pdf",
      "hash": "<sha256 of plaintext>",
      "object": "<n>",
      "size": 183422,
      "mime": "application/pdf",
      "doc_type_id": "…", "tax_year": 2025, "status": "received", "source": "client",
      "filed_by": "kwk", "filed_at": "…", "device_id": "…", "rev": 4,
      "versions": [{ "hash": "…", "object": "…", "size": 1, "created": "…", "created_by": "…", "reason": "…" }],
      "deleted_at": null, "tombstone_reason": null
    }
  ],
  "empty_dirs": ["Payroll/2025"]
}
```

`rel_path` is relative to `folder_rel`, so a client folder rename changes one field.
`empty_dirs` keeps the template tree on restore.

## Encryption

- Per object: XChaCha20-Poly1305, random 24-byte nonce. Object body =
  `b"VF1\0" | key_id (u16 len + utf8) | nonce (24) | ciphertext+tag`.
- `naming_key` and the content key are derived from the firm cabinet key (HKDF-SHA256, distinct
  `info` strings). Key handling is `vf-security::keys`; the firm holds the only copies.
- `key_id` in each object allows rotation; old keys stay in the keyring until no object uses them.

## Write order (push)

1. Upload new `objects/` (skip if the key exists), multipart > 100 MB.
2. Upload new client segments and the cabinet segment.
3. Write `manifest/HEAD` with a conditional put on the previous HEAD's ETag. On conflict: pull,
   merge per CLAUDE.md §Sync rules, retry. HEAD is the commit point; anything not reachable from
   HEAD is garbage a later sweep may remove.

## Restore

`cabinet.json` → passphrase → key → `manifest/HEAD` → segments → for each client: create
`folder_rel`, write `_Vibe/client.json` from `sentinel` verbatim, create `empty_dirs`, download
each current document to `rel_path`, verify SHA-256, then versions into `_Vibe/versions/`. The
restored folder must scan as `bound` in the Filer root scan.
