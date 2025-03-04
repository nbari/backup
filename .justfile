test: clippy
  cargo test -- --nocapture

clippy:
  cargo clippy --all -- -W clippy::all -W clippy::nursery -D warnings

coverage:
  CARGO_INCREMENTAL=0 RUSTFLAGS='-Cinstrument-coverage' LLVM_PROFILE_FILE='coverage-%p-%m.profraw' cargo test
  grcov . --binary-path ./target/debug/deps/ -s . -t html --branch --ignore-not-existing --ignore '../*' --ignore "/*" -o target/coverage/html
  rm -rf *.profraw
  firefox target/coverage/html/index.html&
