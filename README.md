# backup

`backup` is an early-stage Rust CLI for building a secure, content-addressable
backup system.

The current implementation is focused on the metadata engine: scanning files,
hashing content, detecting changes, tracking versions, and storing backup state
in SQLite. File storage, encrypted blob uploads, and restore commands are not
implemented yet.

## Current status

Implemented:

- create named backup definitions
- track configured files and directories
- scan the filesystem
- calculate BLAKE3 content hashes
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

Run a scan:

```bash
backup run mybackup
```

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

Metadata is stored in SQLite under `~/.backup/<name>.db`. Scan errors and
skipped entries are written to `~/.backup/<name>-skipped_files.log` when needed.

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

## Design direction

The goal is to support:

- per-file encryption with unique file keys
- content-addressable encrypted blobs
- deduplication by content hash
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
