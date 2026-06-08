# backup

`backup` is an early-stage Rust CLI for building a secure, content-addressable
backup system.

The implementation covers the metadata engine, key management, and the local
write path: scanning files, computing keyed content identifiers, detecting
changes, tracking versions, and **compressing, encrypting, and writing
content-addressed blobs** to one or more filesystem destinations. Restore and
remote (S3) backends are not implemented yet.

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
- compress (zstd) + encrypt (ChaCha20-Poly1305) each new file's content and write
  the blob to every configured filesystem destination, deduplicated by content id
  (whole-file blobs)
- verify stored blobs against the catalog and repair missing copies
  (copy from a healthy destination, or re-seal from the source file)

Not implemented yet:

- `restore` (the command exists as a placeholder but does not restore anything yet)
- S3 or other remote storage backends (filesystem destinations work; S3 via `s3m`
  is planned)
- chunking / pack files / streaming (content is currently sealed as whole-file blobs)

## Usage

Create a backup definition:

```bash
backup new mybackup -d /home/user1 -d /home/user2
```

Creating a backup prints a **12-word recovery mnemonic once**. Write it down and
store it offline: it is the only secret that can recover the backup, and it is
never written to disk. Creation also writes a `<name>.wkey` cache (see
[Security model](#security-model)).

Change what a backup covers later with `edit` — add or remove directories and
files without recreating it:

```bash
backup edit mybackup -d /home/user3            # add a directory
backup edit mybackup -f /etc/hosts             # add a file
backup edit mybackup --rm-dir /home/user2      # remove a directory
backup edit mybackup --rm-file /etc/hosts      # remove a file
```

Added paths must exist (same checks as `new`); removed paths are matched by
string, so you can drop entries that no longer exist on disk. After editing,
directories are collapsed to non-overlapping parents and any file now covered by
a directory is dropped — the same rules `new` applies. Running `edit mybackup`
with no flags just prints the current configuration.

Run a backup:

```bash
backup run mybackup
```

`run` scans the configured paths and, for each new file content, **compresses
(zstd), encrypts (ChaCha20-Poly1305), and writes a blob** to every configured
destination, keyed by its content id (so identical content is stored once). With
no destination set it records metadata only. Restore is not wired yet (the stored
blobs are already decryptable — proven by tests). Current limits: whole-file
blobs, filesystem destinations only (S3 via `s3m` is planned).

A version is marked **complete** only when its metadata is committed (after all
its blobs are stored), so an **interrupted run is safe**: it leaves an unfinished
version that `view` ignores (showing the last completed snapshot), and a re-run
re-stores any affected content correctly. Orphaned blobs from an interrupted run
are harmless and will be reclaimed by a future `prune`.

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

### Consistency — what to back up

`backup run` takes a **fast, point-in-time snapshot of the filesystem state**,
and the data is uploaded afterwards. Each file is captured as a coherent read,
but the snapshot is **not an atomic image of the whole set** at one instant (no
filesystem/LVM/ZFS snapshot is taken). What this means in practice:

- **Databases and other live, multi-file state:** take an application-level dump
  or backup **first**, then point `backup` at the result. For example run
  `pg_dump` / `mariadb-backup` (or `mariadbbackup`) to a directory, and back up
  that directory — backing up live data files directly is not guaranteed
  consistent.
- **Prefer directories with stable / infrequently-changing data.** That is the
  intended target.
- **Files that change during a run** (e.g. busy logs) are captured as they exist
  **at the moment they are read** — a coherent snapshot of that file at that
  time, just not necessarily the instant `run` started. The next run picks up
  any later changes.

Verify that stored data is still intact:

```bash
backup verify mybackup
```

`run` trusts the catalog when deciding what to upload, so a blob deleted directly
from a destination (e.g. a botched sync, bit-rot cleanup, or an accidental `rm`)
would otherwise go unnoticed. `verify` re-checks every destination against the
catalog and reports any missing blob copies. It reads no secrets.

Repair missing blobs with `--repair`:

```bash
backup verify mybackup --repair
```

For each missing blob, repair restores it the cheapest safe way:

- **copy from a healthy destination** when another destination still has the blob
  (the content key is unchanged, so all copies stay byte-identical), otherwise
- **re-seal from the source file** when the blob is gone from *every* destination:
  the original file is re-read, checked against its content id, re-encrypted with a
  fresh key, written to all destinations, and the catalog key is updated. Re-sealing
  reads the source files and may prompt for the recovery mnemonic.

A blob that is gone from every destination **and** whose source file is missing or
has changed cannot be recovered; `verify --repair` lists those content ids.

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

By default, `backup run` reads `.backupignore` files using **gitignore-style
patterns** to exclude paths from the scan. Put a `.backupignore` in a backed-up
directory; like `.gitignore`, it applies to that directory and everything below
it (nested `.backupignore` files in subdirectories also apply).

Syntax (same as `.gitignore`):

- `name/` — a **trailing slash** matches directories only.
- `target/` — **no leading slash** matches anywhere in the tree (every `target/`
  at any depth).
- `/target/` — a **leading slash anchors** to the directory containing the
  `.backupignore` (only the top-level `target/`).
- `*.log`, `**/cache/` — glob (`*`) and recursive (`**`) wildcards.
- `# ...` — comment; blank lines are ignored.
- `!keep.log` — `!` **negates** a previous pattern (re-include).

### Example: Rust + Terraform

A `.backupignore` that skips Rust build output and Terraform's local state and
plugin cache:

```gitignore
# Rust build artifacts (any crate, any depth)
target/

# Terraform's regenerable plugin/module cache (re-created by `terraform init`)
.terraform/

# common regenerable noise
node_modules/
*.tmp
*.log
```

Use `target/` (no leading slash) so it matches every crate's build dir in a
workspace; use `/target/` if you only want to skip the one at the root. For a
single project rooted at the repo, both behave the same.

Only ignore things you can **regenerate**. For example, don't blanket-ignore
Terraform `*.tfstate` (it can be the source of truth for a local backend) or
`.terraform.lock.hcl` (pins provider versions) — those are usually worth backing
up.

The `.backupignore` file itself is included in the scan unless a pattern
explicitly ignores it.

Also apply `.gitignore` rules on top of `.backupignore`:

```bash
backup run mybackup --gitignore
```

Disable all ignore files (back up everything):

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

## Platform support

`backup` targets **unix only — Linux, macOS, and FreeBSD**. It relies on unix file
semantics (owner-only mode bits for the key cache, and the mode/uid/gid metadata the
data model is built around), so Windows is not supported and the build fails there by
design. Windows users can run it under WSL.

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
