#!/bin/sh
# Local check entry point. Phase 1-A: Rust workspace checks only.
set -e
cd "$(dirname "$0")/.."

if [ -f "$HOME/.cargo/env" ]; then
  . "$HOME/.cargo/env"
fi

cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
