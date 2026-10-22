#!/usr/bin/env bash
# Deployment specs (NEW-61): pack the SDK, unpack the tarball, host examples/vanilla + its dist/ and wasm/ as a static
# site, run the specs in chromium, firefox and webkit. Extra args go to `playwright test` (e.g. --project=webkit).
# Needs wasm + sdk built. Wrap in memory-queue (--need-gb 2); do not nest it.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tgz=$("$here/../package-consumers/pack.sh" "$work")
mkdir "$work/pkg"
tar xzf "$tgz" -C "$work/pkg"
"$here/../package-consumers/stage-vanilla.sh" "$work/pkg/package" "$work/site"
cd "$here/../.."
FOLIARK_SITE=$work/site node_modules/.bin/playwright test -c tests/browser "$@"
