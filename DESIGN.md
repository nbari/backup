# Design

This document defines the architecture and the data-plane logic for `backup`.
It is the reference for implementation; the storage backends themselves are not
built yet.

## 1. Goal & positioning

`backup` is a **client-side-encrypted, deduplicated, versioned** backup tool for
an **untrusted store / shared VM**, built for a specific workflow:

> `backup run <name>` takes a **fast point-in-time snapshot** of the filesystem
> state (metadata only — minutes, even for large trees), and then a **separate,
> resumable phase uploads** the data to one or more destinations (which may take
> hours). The desired state is fixed the instant the snapshot completes; the
> upload merely realizes it. SQLite is used precisely because a metadata-only
> snapshot is fast, transactional, and queryable (§6.7).

Three choices define it:

1. **Asymmetric (public-key) encryption.** The backup host holds only the X25519
   **public key**, so a compromised cron host or a stolen machine **cannot
   decrypt** existing backups — it can capture and upload, not read. Decryption
   requires the BIP-39 mnemonic.
2. **Fast snapshot, deferred multi-destination upload.** Capturing state and
   moving bytes are decoupled (§6.7); uploads can fan out for redundancy.
3. **Browsable catalog.** Paths/names are plaintext **locally** so the catalog is
   a navigable map (and a future web view); only content ids are keyed, and the
   *store* stays zero-knowledge (§7).

## 2. Threat model

- **Trusted:** the machine running `backup`, and the holder of the mnemonic.
- **Untrusted:** the remote store (S3 or a host you don't control) — it may read,
  tamper, reorder, roll back, truncate, or delete what it holds.
- **Guarantees:** content is unreadable without the mnemonic; the store learns
  only opaque blobs and their count/size distribution; tampering and rollback are
  detectable (§7).
- **Out of scope:** an attacker with code execution on the trusted machine while
  a backup runs (can read the live naming key / mnemonic input). See the
  `.wkey` note in §4.

## 3. Cryptography (implemented)

- Primitives: X25519, ChaCha20-Poly1305 (AEAD), HKDF-SHA256, BLAKE3, BIP-39.
- A 12-word mnemonic derives, via HKDF over the full seed, the **X25519 content
  keypair**; only the public key is stored.
- A random **naming key** keys the BLAKE3 content identifiers; it is sealed to
  the public key inside the catalog and cached (owner-only) in `{name}.wkey` so
  routine/cron runs need no secret. A missing `.wkey` prompts for the mnemonic
  (also a recovery self-test).
- Per-unique-file: a random **content key** is generated and **wrapped** to the
  public key (`encrypted_key` + `ephemeral_public_key` in `Files`). This already
  exists; the data plane will use it to encrypt the file's bytes.
- Secrets are held in `Zeroizing`. No convergent encryption (so no
  confirmation-of-file / learn-remaining-info attacks); dedup comes from the
  keyed id, not from deriving keys from content.

## 4. Catalog & identity (implemented)

- Authoritative metadata is a local SQLite catalog (`~/.backup/<name>.db`).
- `Files(hash UNIQUE, encrypted_key, ephemeral_public_key)` — `hash` is the
  **keyed-BLAKE3 content id**; one row per unique content. `Paths` + `FileNames`
  track version intervals (`first_version`, `last_version`); `BackupVersions`
  records runs.
- Files carry a durable id (`FileNames.name_id`) used by `view <name> <id>` and
  the future `restore`.
- Paths/names are plaintext **by design** (browsable map); content ids are keyed
  so a leaked catalog can't confirm known files or correlate content.

## 5. Design properties & tradeoffs

Stated on its own terms — this is not a competition; other tools solve their own
problems well. This is simply what *this* tool deliberately provides, and the
tradeoffs it accepts.

Provides:
- **Write-only backup host** — host holds only the public key; it can capture and
  upload but not decrypt history (§1, §3).
- **Fast snapshot, deferred multi-destination upload** — point-in-time state in
  minutes; bytes moved later, resumably, to one or more targets (§6.7).
- **Browsable local catalog** with restore selection by id / path / version (§4).
- **Zero-knowledge store** — only opaque encrypted blobs and an encrypted catalog
  leave the host (§7).
- **Content-defined dedup + compress-then-encrypt** for changing files (§6).

Accepts (tradeoffs to keep in view):
- **Per-backup dedup scope** (keyed by the naming key) — no cross-tenant
  correlation, but no global dedup across separate backups.
- **Local catalog holds plaintext paths/names** — a stolen catalog reveals the
  file *tree* (names, structure, sizes, version timeline) but **never file
  contents or keys**: the content keys and naming key are sealed to the mnemonic,
  so a DB leak cannot decrypt anything. Low-severity on the host (a local
  attacker can usually enumerate the live filesystem anyway), and the catalog is
  shipped **encrypted** off-host (§7) so the tree is never exposed to the store.
  The residual is that filenames themselves can be sensitive.
- **Asymmetric model** adds a wrapped key per unique chunk vs. a single symmetric
  master key.
- **Single mnemonic** is the only recovery root (loss = total loss).
- The snapshot/upload split introduces a **consistency window** to manage (§6.7).

## 6. Data-plane logic (to build)

The unit of storage is a **content-defined chunk**. A file's content is a
**manifest**: an ordered list of chunk ids. Whole-file storage is the degenerate
"one chunk per file" case — so the **first implementation may ship a single-chunk
(whole-file) chunker and turn on FastCDC later with no schema or crypto change.**

### 6.1 Chunking
Split each file with **FastCDC** (rolling hash, content-defined cut points;
≈1 MiB average, ≈256 KiB min, ≈4 MiB max). Content-defined boundaries
**re-synchronize** after inserts/deletes, so an edit only re-chunks the changed
region and an append only adds tail chunks — unlike fixed-size blocks, where a
1-byte insert shifts and invalidates everything after it. This is what makes
large changing files (VM images, raw DB files, growing logs) cheap to back up.

### 6.2 Dedup
`chunk_id = keyed-BLAKE3(naming_key, chunk)`; the chunk store is keyed by
`chunk_id`. Storing is **idempotent** — a chunk whose id already exists is not
re-uploaded — so identical content across files and versions collapses to one
stored chunk. The dedup check is a **local** catalog lookup (the host holds the
catalog + naming key); no round-trip to the store.

### 6.3 Per-chunk blob — compress then encrypt (#1)
`compress(chunk)` then `encrypt(chunk)`, in that order:
- **Compression (#5):** zstd ~3 by default, recorded with a **1-byte codec tag**
  so codecs are pluggable (e.g. xz/lzma as a higher-ratio option) and
  incompressible chunks can be stored raw.
- **Encryption (#4):** `ChaCha20-Poly1305` by default with a **per-chunk random
  content key**. The **codec tag, cipher tag, and `chunk_id` are bound as
  associated data**, so a tampering store cannot downgrade the codec/cipher or
  swap a chunk undetected. A **1-byte cipher tag** allows an **AES-256-GCM**
  alternative for AES-NI hardware (benchmark ChaCha20 vs AES per platform). The
  content key is **wrapped to the public key** (the same envelope `Files` uses
  today); single-use, so a fixed nonce is safe (XChaCha20-Poly1305 optional).
- **Multiple recipients (planned):** the content key may be wrapped to **several**
  public keys, so a backup can be recovered by more than one keyholder (org
  escrow, a second admin). Cheap in the asymmetric model — one extra wrapped-key
  row per recipient (§6.6), no re-encryption of data. Recipients are configured
  per backup.
- This preserves the asymmetric property — the host stores chunks it cannot
  decrypt. No convergent encryption: dedup comes from the keyed id, not from
  deriving keys from content. (Key **rotation** is correspondingly expensive — it
  re-wraps every chunk key — and is a known limitation, §9.)

### 6.4 File manifest & point-in-time restore
A file version = an ordered list of chunk ids + a whole-file keyed digest (for
verification). Versions **share** unchanged chunks, so each version is a thin
recipe — **not a full copy and not a delta chain**. Any version restores
independently by fetching its chunks and concatenating them; no replaying of
diffs, no dependency on other versions. The `Paths`/`FileNames` version-interval
model is unchanged; only "a file's content" now resolves to a manifest instead
of a single hash.

### 6.5 Storage trait, layout & packs (#3, #8)
```
trait Storage { put(key,bytes); get(key[,range]); exists(key); delete(key); list(prefix); }
```
- **Local backend:** sharded dir (`blobs/ab/cd/<id>`), temp-file + atomic rename
  — the "distributed tree" of #3.
- **S3 backend:** pack objects; multipart for large packs. A FUSE option via
  [mountpoint-s3](https://github.com/awslabs/mountpoint-s3) can be considered for
  mounting, but the native object API is the primary path.
- **Packs (object store):** chunks are aggregated into **pack objects**
  (~16–128 MiB) with an index, to avoid huge object counts (S3 request
  cost/throttling). A blob index maps `chunk_id → (pack_id, offset, length)`,
  recorded in the catalog; `get(key, range)` fetches one chunk from a pack.
- **Streaming & buffer dir (#8):** chunking → compression → encryption →
  pack-build are **streamed**, never materializing a whole compressed+encrypted
  file. A configurable **scratch/buffer directory** (default a temp dir) holds
  in-flight packs so a nearly-full source disk doesn't block backups; the
  FastCDC max-chunk bound (§6.1) is the "max split size".

### 6.6 Schema deltas
- `Chunks(chunk_id PK, size, pack_id, offset, length)` — content-addressed
  encrypted units; the dedup key (replaces `Files.hash UNIQUE`).
- `ChunkKeys(chunk_id, recipient_pubkey, encrypted_key, ephemeral_public_key)` —
  the wrapped content key, **one row per recipient** (§6.3 multi-recipient).
  (Today's inline `encrypted_key`/`ephemeral_public_key` on `Files` generalize to
  this table.)
- `Files(file_id PK, manifest_id UNIQUE, whole_hash, size)` — content identity is
  the manifest (keyed hash of the ordered chunk-id list).
- `FileChunks(file_id, seq, chunk_id, PRIMARY KEY(file_id, seq))` — the ordered recipe.
- **Entry attributes (A):** per-version entry **type** (file / dir / symlink /
  special), **mode**, **uid/gid**, **mtime**, and **symlink target** — on
  `FileNames` or a sibling `EntryMeta` table — so restore reproduces permissions,
  ownership, timestamps, **empty directories**, and symlinks. Hardlinks recorded
  by (device, inode) so they re-link instead of duplicating.
- `Packs(pack_id, object_key, size, …)`.
- Per-version chunk reference counts for prune (§8); a `Config` format-version marker.
- A per-file **stat signature** (size, mtime, inode/ctime) for fast stat-based
  change detection (§6.7), plus a `changed-during-backup` flag.
- Per-`(chunk/pack, destination)` **upload status** (pending/uploaded/verified)
  for the resumable, multi-destination upload state machine (§6.7).
- `Paths` / `BackupVersions` otherwise unchanged.

### 6.7 Snapshot / upload split (#19, #8)
`run` is a **fast, metadata-only snapshot**: walk the tree, `stat` each file
(size, mtime, inode/ctime), and diff against the catalog's stored signatures —
**no file contents are read**, so it completes in minutes even on large trees.
- **Unchanged** files (matching signature) carry forward the previous version's
  manifest/chunks — not re-read.
- **New/changed** files are recorded with their signature and marked **pending**,
  along with their **type and attributes** (§6.8); **symlinks are recorded, not
  followed**.
The version's *file set* is pinned the instant `run` finishes; the first run
treats everything as new (a full backup).

A separate, resumable **upload worker** drains the pending set and does all the
heavy work — read → CDC chunk → hash (keyed id) → dedup (skip known chunks) →
compress → encrypt → upload — fanning out to **one or more destinations** (e.g.
S3 + a local mount) for redundancy (#19). Content hashing and dedup therefore
happen here, **not** at `run`.

- **Upload state machine:** per `(chunk/pack, destination)` status
  (pending / uploaded / verified) in the catalog → resumable across crashes,
  retried per destination, idempotent (a present chunk is skipped). Streaming +
  the scratch dir from §6.5 (#8) feed it.
- **Window reconciliation (decided):** before reading a pending file the worker
  re-`stat`s it. If the signature still matches the snapshot, the stored bytes
  are byte-accurate. If it **changed** during the window, the worker stores the
  bytes **as read now**, records their real content id, and flags the entry
  `changed-during-backup` (the next run reconciles); a vanished file is skipped.
  The run never fails on a moving file.
- **Guarantee:** **per-file** consistency (each file captured as one coherent
  read, verified against its signature) — **not** cross-file atomicity (OS
  snapshots are intentionally avoided). Apps needing a consistent multi-file
  image (e.g. a live database) should dump/quiesce first. An optional `--verify`
  mode re-hashes instead of trusting `stat`, for the rare "content changed but
  mtime didn't" case.
- **Destinations & credentials (C):** each backup's destinations (S3
  endpoint/bucket/region/prefix, mount paths) are configured in the catalog
  `Config`; **credentials are never stored in the catalog** — they come from the
  environment / AWS profile / IAM role.
- A **version is sealed** only once all its chunks are uploaded+verified to the
  destination(s); until then it is pending.

### 6.8 File metadata, directories & special types (A)
A faithful restore needs more than bytes:
- **Attributes** (every entry): type, mode, uid/gid, mtime, and (for symlinks)
  the target — captured at snapshot, re-applied after writing content on restore.
- **Directories**, including **empty** ones, are recorded with their mode/owner,
  so the tree is reproduced exactly (the model is no longer file-only).
- **Symlinks** are stored as the link itself (target string); the scan does
  **not** follow them — avoiding duplicated data and loops.
- **Special files** (device/fifo/socket) are recorded as metadata only (no
  content); **sparse files** restore their holes where the OS supports it.
- **Hardlinks** are detected by (device, inode) and re-created as links on
  restore rather than duplicated (content dedups regardless).

## 7. Zero-knowledge store & integrity

- The store holds **only**: opaque pack/large-file objects, an **encrypted
  catalog/manifest** (sealed to the public key) for disaster recovery, and a
  signed manifest. It never sees plaintext paths/names.
- The local plaintext catalog is the browsable map; the encrypted copy on the
  store is what a fresh machine restores from (mnemonic + store → rebuild).
- **Store layout:** well-known keys so a fresh client can bootstrap — an
  encrypted catalog object, the latest manifest, and a `packs/` prefix. DR =
  fetch manifest + encrypted catalog → decrypt with the mnemonic → restore.
- **Hosted-catalog hook (#6):** because the uploaded `<name>.db` is sealed to the
  public key, a hosting service can store customers' catalogs (and blobs) without
  ever seeing their data — a natural paid offering with no change to the trust
  model.
- **Manifest authenticity & rollback (corrected):** each run writes a manifest
  (Merkle root over chunk ids + file-manifest ids + a monotonic version). The
  host holds only the public key, so it **cannot sign or compute a
  mnemonic-derived MAC**; instead the manifest is MAC'd with a **naming-key-derived
  key** — the host has the naming key (via `.wkey`), a mnemonic-holder can unseal
  it to verify, and the untrusted store cannot forge it. That MAC gives
  **authenticity**; **rollback/truncation** detection comes from **locally
  remembered trusted state** (the client keeps the latest monotonic version/root
  and refuses an older one), since a MAC alone can't stop replay of an old
  genuine manifest. A fresh DR client has no local state, so it authenticates via
  the MAC and trusts the store's latest on first use.
- Inherent leak (all dedup tools): the store sees **chunk count and size
  distribution**.

## 8. Restore, verify, prune & operations

- **Restore** (fills the current placeholder): resolve the target (file id /
  path / whole snapshot at a version) → read each file's manifest (ordered chunk
  ids) → fetch each chunk (pack range or object) → unwrap its content key with
  the mnemonic → decrypt → decompress → concatenate in order → verify the
  whole-file digest → write to `--into <root>` (restore to a defined root, #3)
  or the original path, then **re-apply attributes** (§6.8). Selection by
  id/path/version already exists in `view`.
- **Verify / check (B):** `verify [--deep] [--repair]` — confirm referenced
  chunks/packs exist on each destination (cheap), optionally re-download and
  re-hash (deep), and reconcile catalog↔store drift (e.g. catalog says uploaded
  but the object is missing). Essential against an untrusted/bit-rotting store.
- **Status (C):** `status <name>` — pending vs uploaded bytes/chunks,
  per-destination progress, the last **sealed** version, and recent failures
  (the upload runs for hours, so this matters).
- **Prune + retention (C):** reference-count chunks by version; `prune` applies a
  **GFS-style retention policy** (keep last N; hourly/daily/weekly/monthly/yearly;
  keep-within a duration), then mark-and-sweeps unreferenced chunks and repacks
  packs to reclaim space.
- **Locking (B):** a catalog lock prevents concurrent `run`/`prune` on the same
  backup from corrupting state.

## 9. Future / deferred

- **Async change tracking / `watch` mode (#6):** a Dropbox-like daemon that
  records filesystem changes continuously and digests them on a schedule/priority
  (vs. today's on-demand `run`). A future operational mode on top of the same
  data plane.
- **Nice-to-have (later):** snapshot tags/labels (name a version "pre-upgrade")
  alongside numeric versions; upload throttling, retry/backoff and parallelism
  knobs; S3 storage-class / lifecycle (cold tiers); and fixing the pre-existing
  `-c/--config` bug (`run`/`view`/`edit` ignore it and always use `~/.backup`).
- **Known limitation — key rotation:** the asymmetric model makes rotating the
  keypair expensive (re-wrap every chunk key); multi-recipient (§6.3) covers the
  "more than one keyholder" need without rotation.
- **Non-goal — cross-backup / cross-tenant global dedup:** rejected for privacy;
  dedup is per-backup, keyed by the naming key.
- **Non-goal — OS-keyring / TPM-backed naming-key storage** (would harden against
  an active local-root adversary).
- **Non-goal — encrypting path/name columns** (kept plaintext locally for the map).

## 10. Current status

Built: metadata engine, keyed ids, versioning, key management (sealed naming key
+ `.wkey`), `new`/`edit`/`run`/`show`/`view`, `restore` placeholder.
Not built: chunking (FastCDC; a single-chunk/whole-file chunker is an acceptable
first step), file manifests, compression, content encryption of chunk bytes,
blob/pack storage, local & S3 backends, encrypted catalog/manifest upload,
manifest TAM, restore, prune.

Note — behavior change from today: the current `run` hashes file content during
the scan. The target model (§6.7) makes `run` **metadata-only** (stat-based
change detection) and moves content hashing/CDC/dedup into the upload phase, so
each changed file is read once. This refactor lands with the data plane.

## 11. Issue backlog mapping

This document consolidates the early design issues; each maps to a section here.

| Issue | Topic | Section | Status |
|---|---|---|---|
| #1 | compress then encrypt | §6.3 | decided |
| #3 | backup layout / restore-to-root / distributed tree | §6.5, §8 | decided |
| #4 | ChaCha20-Poly1305 default, AES-256 fallback, chunk encrypt | §6.3 | decided |
| #5 | zstd compression (xz optional) | §6.3 | decided |
| #6 | track changes async + hosted encrypted catalog | §9 (watch), §7 (hosting) | designed / future |
| #8 | streaming to S3, buffer dir, split size | §6.5, §6.7 | designed |
| #18 | list files with id to restore | `view`/`browse` | **implemented** |
| #19 | back up to multiple locations (S3 + mount) | §6.7 | designed |
