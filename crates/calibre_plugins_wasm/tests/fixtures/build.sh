#!/usr/bin/env bash
# Rebuilds probe_plugin.wasm. Requires: rustup target add wasm32-unknown-unknown
set -euo pipefail
cd "$(dirname "$0")/probe_plugin"
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/probe_plugin.wasm ../probe_plugin.wasm
echo "wrote $(cd .. && pwd)/probe_plugin.wasm"
