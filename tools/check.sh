#!/bin/sh
# The checks every commit passes (the pre-commit hook runs them): formatting, clippy with warnings as
# errors, and the tests, the snapshots among them. About a minute.
#
#   tools/check.sh
set -eu
cd "$(dirname "$0")/../clamshell"
cargo fmt --check
cargo clippy --workspace --all-targets --quiet -- -D warnings
cargo test --workspace --quiet
