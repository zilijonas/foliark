#!/usr/bin/env bash
# Regenerate the ts-rs bindings into a temp dir and diff them against packages/sdk/src/generated/.
# Exit 0 = in sync; non-zero = Rust DTOs and committed TS drifted (run `cargo test -p foliark-model`, commit the result).
set -euo pipefail
cd "$(dirname "$0")/.."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
TS_RS_EXPORT_DIR="$tmp" cargo test -q --locked -p foliark-model >/dev/null
diff -r "$tmp" packages/sdk/src/generated && echo "ts parity: ok ($(ls "$tmp" | wc -l | tr -d ' ') files)"
