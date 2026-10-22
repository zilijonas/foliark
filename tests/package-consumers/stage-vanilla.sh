#!/usr/bin/env bash
# stage-vanilla.sh <installed @foliark/sdk dir> <site dir>: the vanilla example plus the package's dist/, wasm/ and cmaps/ as sdk/
# (the SDK finds its CMap pack at ../cmaps/ from dist/worker.js).
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$2/sdk"
cp "$root"/examples/vanilla/{index.html,main.js,ui.js,flow.js} "$2/"
cp -R "$1/dist" "$1/wasm" "$1/cmaps" "$2/sdk/"
