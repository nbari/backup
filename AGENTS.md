# Repository Guidelines

## Project Structure & Module Organization

This is a Rust 2024 CLI project. The binary entry point is `src/bin/backup.rs`,
with shared modules exported from `src/lib.rs`.

- `src/cli/commands`: clap command definitions.
- `src/cli/dispatch`: command-to-action mapping.
- `src/cli/actions`: thin CLI adapters and terminal output.
- `src/engine`: backup workflows such as create, run, and show.
- `src/db`: SQLite metadata catalog and schema logic.
- `src/storage`: future local and S3-compatible storage abstractions.
- `src/utils`: crypto, hashing, formatting, and small helpers.

Tests are colocated with the modules they exercise using `#[cfg(test)]`.

## Build, Test, and Development Commands

- `cargo build`: compile in debug mode.
- `cargo run -- new mybackup -d /path/to/source`: create a backup definition.
- `cargo run -- run mybackup`: scan and update SQLite metadata.
- `cargo test`: run unit and doc tests.
- `cargo clippy --all-targets --all-features`: run required lint checks.
- `cargo fmt --all -- --check`: verify formatting.
- `just test`: run clippy, then tests with `--nocapture`.
- `just coverage`: generate HTML coverage under `target/coverage/html`.

## Coding Style & Naming Conventions

Use standard `rustfmt` formatting. Follow Rust naming conventions:
`snake_case` for modules/functions, `PascalCase` for types, and
`SCREAMING_SNAKE_CASE` for constants.

Strict lints are configured in `Cargo.toml`, including `clippy::pedantic`,
`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, and
`large_stack_arrays`. Prefer explicit error propagation with `anyhow::Result`
and `?`.

## Testing Guidelines

Add focused tests near changed code. Database tests should assert version and
restore semantics explicitly: unchanged files, modified files, deletions,
reverted content, deduplication, and ignore-rule behavior.

## Architecture Notes

SQLite metadata correctness is the priority. The project currently scans files,
hashes content, tracks versions, and records restore metadata. Blob storage,
encrypted content persistence, upload queues, and restore commands are not
complete yet.

Default scan ignores come from `.backupignore`. `.gitignore` is only used with
`--gitignore`; `--no-ignore` disables ignore files. Progress is shown by
default, while `-q` suppresses user-facing output.

## Commit & Pull Request Guidelines

Use short, imperative commit subjects. PRs should describe behavior changes,
mention schema or migration impact, and list verification commands such as
`cargo test`, `cargo clippy --all-targets --all-features`, and
`cargo fmt --all -- --check`.
