#!/usr/bin/env bash
# Pinned, bounded fuzz run: every target for SECS seconds (default 60) with a fixed libFuzzer
# seed, 2 s per-input timeout, 2 GB RSS cap. Exits non-zero if any target crashes, hangs or
# runs out of memory (the first artifact of each failing target is printed).
#   tools/fuzz-smoke/run.sh [target...]        FUZZ_SECS=120  FUZZ_RUNS=200000 (exec-exact)
# On the owner's Mac mini the whole run goes through the memory queue (4 GB); without ~/ai/bin/memory-queue it runs directly. Needs `rustup toolchain install nightly`
# and `cargo install cargo-fuzz --locked`.
set -uo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
if [ -z "${FOLIARK_FUZZ_QUEUED:-}" ] && [ -x ~/ai/bin/memory-queue ]; then
  export FOLIARK_FUZZ_QUEUED=1
  exec ~/ai/bin/memory-queue run --owner "${MEMORY_QUEUE_OWNER:-foliark-fuzz-smoke}" \
    --label 'foliark fuzz-smoke' --need-gb 4 --restartable --wait 1800 -- "$0" "$@"
fi
source ~/.cargo/env 2>/dev/null || true
# Sanitizer builds of every crate at full parallelism page the Mac mini out (twice an emergency stop); two jobs fit.
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
cd "$root" || exit 2
targets=("$@")
[ ${#targets[@]} -gt 0 ] || targets=(open page_model extract render roundtrip content_reemit geometry)
secs="${FUZZ_SECS:-60}"
stop="-max_total_time=$secs"
[ -z "${FUZZ_RUNS:-}" ] || stop="-runs=$FUZZ_RUNS"
tools/fuzz-smoke/seed.sh || exit 2
cargo +nightly fuzz build || exit 2
failed=()
for t in "${targets[@]}"; do
  echo "== $t ($stop)"
  cargo +nightly fuzz run "$t" -- "$stop" -seed=1 -max_len=262144 -timeout=2 \
    -rss_limit_mb=2048 -malloc_limit_mb=1024 -dict=fuzz/pdf.dict -print_final_stats=1 || {
    failed+=("$t")
    ls fuzz/artifacts/"$t" 2>/dev/null | head -3 | sed "s|^|   artifact fuzz/artifacts/$t/|"
  }
done
if [ ${#failed[@]} -gt 0 ]; then
  echo "FAILED: ${failed[*]}"
  exit 1
fi
echo "all ${#targets[@]} targets clean"
