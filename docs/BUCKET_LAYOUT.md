# Vibe Files bucket layout — v1

The contract `vf-sync` implements (P11; remote-mode objects are v2, PA-17). Vibe Files' own layout; Practice Manager is not
consulted (owner 2026-11-02). Status: **v1 as built in P11 (2026-11-03)**, with the amendments marked *P11* below; any
further change bumps `layout_ver` and ships a migration.

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
| `cabinet.json` | no | `{layout_ver: 1, cabinet_id, firm_id, created, key_ids: ["k-01", …], wrapped: {"k-01": <WrappedKey>}, object_lock_days}`. *P11:* `wrapped` is the firm sync key wrapped by the sync passphrase (Argon2id params and salt inside, ChaCha20-Poly1305, the cabinet id as associated data): no secret in the clear, and a bare PC restores from the bucket plus the passphrase alone. `object_lock_days` records the firm's B2 Object Lock choice (display only). |
| `manifest/HEAD` | yes | `{rev, written_at, device_id, device_name, writer, segments: {<client_id>: <segment_rev>}, cabinet_segment_rev}`. Written **last** on every push. *P11:* `writer` is random per write (see Write order). |
| `manifest/clients/<client_id>/<segment_rev>` | yes | One client's segment (below). Immutable; a new rev is a new object. |
| `manifest/cabinet/<rev>` | yes | *P11:* `{segment_rev, files: {<_Vibe-relative path>: <text>}, documents: [...], written_by}`: the cabinet's shared master files (`cabinet.json`, `clients.json`, `relationships.json`, `template.json`, `clients.vf.json`, `doc_types.json`, `retention.json`, `legal_holds.json`, `extract_aliases.json`, `trash_pin.json`, Filer's `config.json` and `rules.json`, `parsers/*.toml`) and the documents outside client folders (paths cabinet-relative). |
| `objects/<n[0..2]>/<n>` | yes | File content. `n = hex(HMAC-SHA256(naming_key, sha256(plaintext)))`. Same content → same object (dedupe across clients, versions, devices). |
| `thumbs/<n[0..2]>/<n>` | yes | Page-1 thumbnail of object `n` (remote mode: PA-17, v2). |
| `locks/<doc_id>` | yes | Check-out lock `{doc_id, user, device_id, since, lease_until}`. Deleted on check-in. |
| `devices/<device_id>` | yes | `{device_id, name, mode, last_seen, acked_rev}`. Tombstoned objects are removed only when every live device's `acked_rev` ≥ the tombstone's rev (and retention allows). |
| `catalog/<rev>` | yes | Catalog snapshot for remote mode (PA-17, v2; not written in v1). Without FTS tables in v1. |
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
  "sentinel": "<the SentinelV1 file's exact text (P11: a string, so a restore writes the same bytes)>",
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

`rel_path` is relative to `folder_rel`, so a client folder rename changes one field. *P11:* each
entry also carries `tags`; tombstones stay listed (with `deleted_at`, `tombstone_reason`) once a
document has been sent; `written_by` names the device. Per-document `docmeta.jsonl` lines are not
sent: the receiving side writes them from the entry (Q-22).
`empty_dirs` keeps the template tree on restore.

## Encryption

- Per object: XChaCha20-Poly1305, random 24-byte nonce. Object body =
  `b"VF1\0" | key_id (u16 LE len + utf8) | nonce (24) | ciphertext+tag`. *P11:* the object's key
  in the bucket (`objects/ab/<n>`, `manifest/HEAD`, …) is the associated data, so objects cannot
  be swapped. HKDF salt `vibe-files sync v1`, `info` = `content` / `naming`; the firm sync key is
  `vf_sync::setup` (wrapped by `vf_security::keys`).
- `naming_key` and the content key are derived from the firm cabinet key (HKDF-SHA256, distinct
  `info` strings). Key handling is `vf-security::keys`; the firm holds the only copies.
- `key_id` in each object allows rotation; old keys stay in the keyring until no object uses them.

## Write order (push)

1. Upload new `objects/` (skip if the key exists), multipart > 100 MB.
2. Upload new client segments and the cabinet segment.
3. Write `manifest/HEAD`. *P11:* B2's S3 API rejects conditional PUT headers (`If-None-Match`;
   `If-Match` unconfirmed), so there is no compare-and-swap. A push first pulls when HEAD's rev
   is ahead of its own, writes HEAD with a fresh random `writer`, and reads it back. When another
   writer's HEAD is there instead, it pulls and retries (3 tries). The window between write and
   read is small, and v1 runs one Intake writer at a time; the lock-based guard is PA-16. HEAD is
   the commit point; anything not reachable from HEAD is garbage a later sweep may remove.
4. *P11:* existing objects are found with a HEAD request and skipped (same content, same name),
   which also lets an interrupted push resume. Disposed documents' objects are deleted only once
   every `devices/<id>` has `acked_rev` ≥ HEAD's rev and no live document or version shares them.

## Restore

`cabinet.json` → passphrase → key → `manifest/HEAD` → segments → for each client: create
`folder_rel`, write `_Vibe/client.json` from `sentinel` verbatim, create `empty_dirs`, download
each current document to `rel_path`, verify SHA-256, then versions into `_Vibe/versions/`. The
restored folder must scan as `bound` in the Filer root scan.
