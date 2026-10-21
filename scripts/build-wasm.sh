#!/usr/bin/env bash
# Build the wasm facade and generate the JS glue into packages/sdk/wasm/ (gitignored).
# Needs wasm-bindgen-cli 0.2.129 on PATH (must equal the wasm-bindgen crate pin).
set -euo pipefail
cd "$(dirname "$0")/.."
[ "$(wasm-bindgen --version)" = "wasm-bindgen 0.2.129" ] || { echo "wasm-bindgen-cli 0.2.129 required" >&2; exit 1; }
cargo build --release --locked --target wasm32-unknown-unknown -p foliark-wasm
rm -rf packages/sdk/wasm
wasm-bindgen --target web --out-dir packages/sdk/wasm target/wasm32-unknown-unknown/release/foliark_wasm.wasm
wasm=packages/sdk/wasm/foliark_wasm_bg.wasm
echo "wasm raw $(wc -c < "$wasm") B, gzip -9 $(gzip -9 -c "$wasm" | wc -c) B"
