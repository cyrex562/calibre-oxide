#!/usr/bin/env bash
# Rebuilds the checked-in .wasm fixtures.
# Requires: rustup target add wasm32-unknown-unknown
set -euo pipefail
cd "$(dirname "$0")"
for p in probe_plugin banner_plugin; do
  (cd "$p" && cargo build --release --target wasm32-unknown-unknown)
  cp "$p/target/wasm32-unknown-unknown/release/$p.wasm" "./$p.wasm"
  echo "wrote $(pwd)/$p.wasm"
done
