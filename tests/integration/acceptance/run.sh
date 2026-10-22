#!/usr/bin/env bash
# Independent cumulative acceptance (NEW-59): build the candidate, pack it, install the TARBALL into a temp project, drive every case through
# it (real wasm, @zilijonas/foliark/inline), judge the exported bytes with the independent oracles. Exit 0 iff no failure and no missed negative control.
# Run it once under the memory queue (it does not call the queue itself):
#   ~/ai/bin/memory-queue run --owner <id> --label 'foliark acceptance' --need-gb 4 --restartable --wait 1800 -- tests/integration/acceptance/run.sh
# SKIP_BUILD=1 reuses the current packages/sdk/{dist,wasm}.
set -euo pipefail
cd "$(dirname "$0")/../../.."
source ~/.cargo/env 2>/dev/null || true
ROOT=$PWD
OUT=${ACCEPT_OUT:-$ROOT/target/acceptance}
PY=$ROOT/tests/oracles/.venv/bin/python
rm -rf "$OUT" && mkdir -p "$OUT/pack" "$OUT/project" "$OUT/out"
if [ "${SKIP_BUILD:-0}" != 1 ]; then scripts/build-wasm.sh >"$OUT/build.log" 2>&1; pnpm build:sdk >>"$OUT/build.log" 2>&1; fi
TGZ=$(tests/package-consumers/pack.sh "$OUT/pack")
echo "candidate: $TGZ sha256 $(shasum -a 256 "$TGZ" | cut -c1-16)"
( cd "$OUT/project" && echo '{"name":"acceptance-consumer","private":true,"type":"module"}' > package.json \
  && npm install --offline --no-audit --no-fund --silent "$TGZ" && cp "$ROOT/tests/integration/acceptance/drive.mjs" . )
$PY tests/integration/acceptance/judge.py prep "$OUT/out"
( cd "$OUT/project" && FONT_TTF=$ROOT/crates/foliark-text/tests/data/LiberationSans-Regular.ttf node drive.mjs "$OUT/out/cases.json" "$OUT/out" )
$PY tests/integration/acceptance/judge.py judge "$OUT/out" 2>&1 || RC=$?
exit ${RC:-0}
