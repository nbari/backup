# Repository Guidelines

## Project Structure & Module Organization

This is a Rust 2024 CLI project. The binary entry point is `src/bin/backup.rs`; shared code is exported through `src/lib.rs`. CLI parsing lives in `src/cli/commands`, command dispatch in `src/cli/dispatch`, and command behavior in `src/cli/actions`. Utility modules are in `src/utils`, including hashing, crypto, database helpers, formatting, and exclusions.

Tests are currently colocated with the modules they exercise using `#[cfg(test)]`. There is no separate `tests/` directory yet. SQLite metadata is the core implementation surface; storage, uploads, and restore are intentionally not fully implemented.

## Build, Test, and Development Commands

- `cargo build`: compile the project in debug mode.
- `cargo run -- new mybackup -d /path/to/source`: run the CLI locally.
- `cargo test`: run unit and doc tests.
- `cargo clippy --all-targets --all-features`: run the required lint suite.
- `cargo fmt --all -- --check`: verify Rust formatting.
- `just test`: run clippy, then tests with `--nocapture`.
- `just coverage`: generate HTML coverage under `target/coverage/html`.

Run clippy before submitting changes; warnings are denied.

## Coding Style & Naming Conventions

Use standard `rustfmt` formatting with 4-space indentation. Follow idiomatic Rust naming: modules and functions in `snake_case`, types and enum variants in `PascalCase`, constants in `SCREAMING_SNAKE_CASE`.

The project enforces strict lints in `Cargo.toml`, including `clippy::pedantic`, `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, and `large_stack_arrays`. Prefer explicit error propagation with `anyhow::Result` and `?`. Avoid unchecked indexing; use safe accessors and return meaningful errors.

## Testing Guidelines

Add focused unit tests near the code being changed. Use `Result<()>` test functions when setup can fail, and avoid `unwrap()` or `expect()`. For database behavior, prefer in-memory SQLite schemas and assert restore/version semantics explicitly, such as unchanged files, modified files, deletions, and reverted content.

## Commit & Pull Request Guidelines

Recent commits use short, imperative summaries such as `coverage workflow`, `cargo bump`, and `dry-run print`. Keep commit subjects concise and specific.

Pull requests should describe the behavior change, mention schema or migration impact when relevant, and include the commands run, especially `cargo test`, `cargo clippy --all-targets --all-features`, and `cargo fmt --all -- --check`.

## Architecture Notes

Treat SQLite metadata correctness as the priority. Do not assume blob storage, encryption-at-rest for file contents, upload queues, or restore workflows are complete unless the code implements them. Preserve compatibility with the version/history model before adding storage features.
