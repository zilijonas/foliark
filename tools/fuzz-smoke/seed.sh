#!/usr/bin/env bash
# Fill fuzz/corpus/<target>/ with symlinks to the synthetic fixtures (no duplicated bytes) plus
# a few tiny hand-made content streams and geometry vectors. Idempotent.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
corpus="$root/fuzz/corpus"
pdf_targets=(open page_model extract render roundtrip)
for t in "${pdf_targets[@]}"; do
  mkdir -p "$corpus/$t"
  for f in "$root"/tests/fixtures/hddn/*.pdf "$root"/tests/fixtures/generated/*.pdf \
           "$root"/tests/regressions/*.pdf; do
    [ -e "$f" ] || continue
    ln -sf "$f" "$corpus/$t/$(basename "$f")"
  done
done
mkdir -p "$corpus/content_reemit" "$corpus/geometry"
printf '\0q 1 0 0 1 10 20 cm BT /F1 12 Tf 72 700 Td (Hello) Tj [(A) -20 (B)] TJ ET Q\n' >"$corpus/content_reemit/text.bin"
printf '\0/Im1 Do BI /W 2 /H 2 /BPC 8 /CS /G ID abcd\nEI 0 0 100 100 re f %% c\n' >"$corpus/content_reemit/inline-image.bin"
printf '\x40q (unterminated Q <41 [1 2 << /A /B >> ] BT ET\n' >"$corpus/content_reemit/junk.bin"
python3 - "$corpus/geometry" <<'PY'
import struct, sys
d = sys.argv[1]
def vec(name, media, crop, uu, rot):
    open(f"{d}/{name}.bin", "wb").write(struct.pack("<4d4ddq", *media, *crop, uu, rot))
vec("letter", (0, 0, 612, 792), (0, 0, 0, 0), 1.0, 0)
vec("rotated-crop", (0, 0, 612, 792), (10, 10, 300, 400), 2.0, 90)
vec("huge", (-1e308, -1e308, 1e308, 1e308), (0, 0, 0, 0), 1e10, 270)
vec("flipped", (612, 792, 0, 0), (0, 0, 0, 0), 1.0, -90)
PY
