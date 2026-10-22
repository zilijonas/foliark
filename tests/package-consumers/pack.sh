#!/usr/bin/env bash
# pack.sh <outdir>: pack @foliark/sdk privately into <outdir> and print the tarball path. Nothing is published.
# Needs the built wasm glue and dist (scripts/build-wasm.sh, pnpm build:sdk).
set -euo pipefail
out=$(cd "$1" && pwd)
cd "$(dirname "$0")/../../packages/sdk"
pnpm pack --pack-destination "$out" >/dev/null
echo "$out/foliark-sdk-$(node -p 'require("./package.json").version').tgz"
