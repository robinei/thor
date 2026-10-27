#!/bin/sh
# The checks every commit passes (the pre-commit hook runs them): formatting, clippy with warnings as
# errors, and the tests, the snapshots among them. About a minute.
#
#   tools/check.sh
set -eu
cd "$(dirname "$0")/../clamshell"
cargo fmt --check
cargo clippy --workspace --all-targets --quiet -- -D warnings
# (The development build too: --snapshot and the demo library.)
cargo clippy --package clamshell --features dev --quiet -- -D warnings
# The device's build, without the nested session (the tests and examples too).
cargo clippy --workspace --all-targets --no-default-features --quiet -- -D warnings
# (And the release build, where code under cfg(debug_assertions) is gone.)
cargo clippy --workspace --release --quiet -- -D warnings
cargo test --workspace --quiet
# The docs, private items too (a broken link is a warning, so an error here).
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --quiet
