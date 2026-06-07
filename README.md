# backup

`backup` is an early-stage Rust CLI for building a secure, content-addressable
backup system.

The current implementation is focused on the metadata engine and key
management: scanning files, computing keyed content identifiers, detecting
changes, tracking versions, and storing backup state in SQLite. File-content
storage, encrypted blob uploads, and restore commands are not implemented yet.

## Current status

Implemented:

- create named backup definitions
- generate a BIP-39 recovery mnemonic and derive an X25519 key from it
  (only the public key is stored; the mnemonic recovers everything)
- track configured files and directories
- scan the filesystem
- compute **keyed** BLAKE3 content identifiers (opaque without the naming key)
- per-file content keys, wrapped (sealed) to the backup public key
- detect new, changed, unchanged, and deleted files
- store version history in SQLite
- keep enough metadata to query historical snapshots

Not implemented yet:

- copying file contents
- encrypting file contents
- uploading blobs
- restore command
- S3 or other storage backends

## Usage

Create a backup definition:

```bash
backup new mybackup -d /home/user1 -d /home/user2
```

Creating a backup prints a **12-word recovery mnemonic once**. Write it down and
store it offline: it is the only secret that can recover the backup, and it is
never written to disk. Creation also writes a `<name>.wkey` cache (see
[Security model](#security-model)).

Run a scan:

```bash
backup run mybackup
```

Routine runs read the local `<name>.wkey` cache and need no secret, so they work
unattended (for example from `cron`). If that cache file is missing, `run`
prompts for the recovery mnemonic to unlock the backup and rewrites the cache.

Runs show scan progress by default, including active hashing workers and the
SQLite metadata write phase. Use `-q` or `--quiet` to suppress progress and
summary output.

Preview a run without updating metadata:

```bash
backup run mybackup --dry-run
```

Show configured backups:

```bash
backup show
```

Browse the backed-up file tree of a snapshot (alias: `browse`):

```bash
backup view mybackup
```

`view` reads the versioned metadata and prints the actual captured file tree.
By default it shows the latest snapshot to a depth of 2, annotating deeper
directories with their file count. Each file is shown with a stable id (`[N]`)
in the left gutter. Use `-d`/`--depth N` (`0` for the full tree) and
`--version V` to change what is shown:

```bash
backup view mybackup -d 0               # full tree, with file ids
backup view mybackup --version 3        # an older snapshot
```

Pass a target to act on a specific entry:

```bash
backup view mybackup 7                   # resolve file id 7 (also "#7")
backup view mybackup /home/user/docs     # drill into a directory subtree
```

A numeric target is a **file id** — `view` prints its full path (and, once
restore lands, restores it). The id is the file's stable database key, so it
stays valid across listings and depths for that version. An absolute-path target
lists that directory's subtree. (Directories are addressed by path, not id.)

The path must be the **full absolute path** as stored (e.g.
`/home/nbari/projects/rust`, not `/rust`); matching is exact, with no partial or
fuzzy resolution — use the file ids for a shorter handle.

Metadata is stored in SQLite under `~/.backup/<name>.db`, with the naming-key
cache alongside it as `~/.backup/<name>.wkey`. Scan errors and skipped entries
are written to `~/.backup/<name>-skipped_files.log` when needed.

## Ignore rules

By default, `backup run` reads `.backupignore` files and uses gitignore-style
patterns:

```gitignore
target/
*.tmp
node_modules/
```

The `.backupignore` file itself is included in the scan unless it is explicitly
ignored.

Use `.gitignore` rules in addition to `.backupignore`:

```bash
backup run mybackup --gitignore
```

Disable ignore files completely:

```bash
backup run mybackup --no-ignore
```

## Security model

Backups are designed around an untrusted remote store: the backup host holds
only public/write material, never the content-decryption key.

- **Recovery root:** a 12-word BIP-39 mnemonic. An X25519 content keypair is
  derived from it with HKDF-SHA256. Only the public key is stored; the private
  key is derived transiently from the mnemonic and never persisted.
- **Naming key:** a random key that keys the BLAKE3 content identifiers, so the
  ids stored in the catalog are opaque to anyone without it (no known-file
  confirmation, no cross-backup correlation). The naming key is **sealed to the
  public key** inside the `.db`, and cached in `<name>.wkey` (owner-only) so
  routine runs stay unattended. Deleting `<name>.wkey` forces a one-time
  mnemonic prompt, which doubles as a recovery-phrase self-test.
- **What stays plaintext (by design):** file paths and names in the catalog, to
  keep the metadata browsable as a map. Content download/decrypt is gated behind
  the mnemonic.
- **Limitation:** because `<name>.wkey` is a persistent plaintext cache, a local
  root user can read the naming key and de-anonymize the identifiers. Content is
  still never decryptable without the mnemonic. Keyring/TPM-backed key storage
  is future work.

Primitives: X25519, ChaCha20-Poly1305 (AEAD), HKDF-SHA256, BLAKE3 (keyed), and
BIP-39. Secret material is wrapped in `Zeroizing` so it is cleared from memory.

## Design direction

The goal is to support:

- per-file encryption with unique file keys
- content-addressable encrypted blobs
- deduplication by keyed content identifier
- complete version history
- deleted-file tracking
- point-in-time restore
- future storage backends such as S3, MinIO, B2, and local filesystem storage

SQLite is currently the authoritative metadata store. Storage backends should be
blob repositories only; metadata should remain the source of truth.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features
cargo test
```

The project uses Rust 2024 and strict lint settings. Warnings, `unwrap()`,
`expect()`, panics, unchecked indexing, and large stack arrays are denied.

Diagnostic tracing is disabled by default. Use `RUST_LOG`, for example
`RUST_LOG=backup=debug cargo run -- run mybackup`, when debugging internals.

## Packaging

Release workflows build archive artifacts and Linux packages. Package metadata
for `.deb` and `.rpm` output is defined in `Cargo.toml`.
