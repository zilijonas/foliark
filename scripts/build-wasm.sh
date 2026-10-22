#!/usr/bin/env bash
# Build the wasm facade, generate the JS glue into packages/sdk/wasm/ (gitignored) and optimise the wasm with wasm-opt.
# Needs wasm-bindgen-cli 0.2.129 on PATH (must equal the wasm-bindgen crate pin).
set -euo pipefail
cd "$(dirname "$0")/.."
[ "$(wasm-bindgen --version)" = "wasm-bindgen 0.2.129" ] || { echo "wasm-bindgen-cli 0.2.129 required" >&2; exit 1; }
cargo build --release --locked --target wasm32-unknown-unknown -p foliark-wasm
rm -rf packages/sdk/wasm
wasm-bindgen --target web --out-dir packages/sdk/wasm target/wasm32-unknown-unknown/release/foliark_wasm.wasm
wasm=packages/sdk/wasm/foliark_wasm_bg.wasm
size() { echo "$1 raw $(wc -c < "$wasm") B, gzip -9 $(gzip -9 -n -c "$wasm" | wc -c) B"; }
size "wasm-bindgen"
# binaryen from npm (root devDependency). -O1 measured best on the wire: -O3/-Oz/-Os/-O2 shrink raw 7-9% but grow gzip and
# brotli ~3% (docs/deployment.md). Features are the ones rustc emits for wasm32 with simd128; simd must stay enabled.
node_modules/.bin/wasm-opt -O1 --enable-simd --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
  --enable-mutable-globals --enable-reference-types --enable-multivalue "$wasm" -o "$wasm.opt"
mv "$wasm.opt" "$wasm"
size "wasm-opt -O1"
