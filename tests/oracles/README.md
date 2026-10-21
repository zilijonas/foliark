# Independent PDF oracles (NEW-7)

The Foliark engine never grades itself. These checks read an OUTPUT PDF with three unrelated parsers
(pypdf, pikepdf/qpdf, PyMuPDF/MuPDF) and know nothing about `foliark-*` crates. Dev-only: none of this ships.

## Setup and run
```sh
python3 -m venv tests/oracles/.venv && tests/oracles/.venv/bin/pip install --only-binary=:all: -r tests/oracles/requirements.txt
tests/oracles/.venv/bin/python tests/oracles/controls.py          # self-test; exit 0 only if every control behaves
tests/oracles/.venv/bin/python tests/oracles/objects.py out.pdf --forbid SECRET_A --forbid SECRET_B [--allow metadata] [--password pw]
tests/oracles/.venv/bin/python tests/oracles/objects.py out.pdf --spec spec.json   # adds "image_regions", "vector_regions", "cap"
```
`import objects; objects.check_isolated(path, spec)` returns a list of findings (`CODE: detail`); empty means pass.
Each check runs in a subprocess with a timeout (default 120 s) and a per-stream decode cap (default 64 MiB).
Run `controls.py` first; an oracle that has not just caught the leaky controls proves nothing.

## What each check proves (and finding code)
| Check | Proves | Code |
|---|---|---|
| Text via pypdf AND PyMuPDF | forbidden strings are not extractable by two independent extractors (whitespace-insensitive) | TEXT_PYPDF, TEXT_FITZ |
| Raw file bytes | forbidden string (ASCII, UTF-16BE/LE) is not anywhere in the file bytes | BYTES_RAW |
| Every decoded stream | not in any stream of ANY object, reachable or not, incl. text operands of Tj/TJ (hex strings, kerning splits) | STREAM |
| Every string and name object | not in any PDF string/name/key, e.g. /Info, /ActualText, /Alt, JS | STRING |
| Revisions | exactly one `%%EOF` and `startxref`, no `/Prev`, no duplicate object definitions | REVISION |
| Orphans | every xref object is reachable from the trailer; no raw objects missing from the xref | ORPHAN |
| Features | no /Info values, /Metadata (XMP), /EmbeddedFiles, /JavaScript, /JS, /OpenAction, /AA, /AcroForm, /OCProperties, /Encrypt unless `allow`ed | FEATURE |
| Image pixels | every image occurrence (page, Form XObject, any CTM: rotated, scaled, mirrored, any page /Rotate) under a display-space rect has one colour in its colour samples AND in its SMask / stencil `/Mask` samples (optionally an exact `color`); one finding per occurrence, naming the other pages that share the xref (a surviving occurrence still holding the pixels = missing clone) | IMAGE_REGION |
| Vector paths | no filled or stroked path geometry (page or Form content, PyMuPDF `get_drawings`) reaches into a display-space rect beyond `tol` (0.25 pt) except the declared `fill` (RGB 0-255, must be the last painted hit and cover the region) and paths inside a declared `allow` rect | VECTOR_REGION |
| Never silent | decode error, truncated flate, unknown filter, decode cap, unreadable file, extractor failure, crash, timeout | DECODE, UNSUPPORTED, CAP, OPEN, TIMEOUT, CRASH |

`controls.py` builds 24 leaky outputs (overlay over live text, Info, XMP, attachment, prior revision, secret only in an unused Form,
hidden OCG, image pixels under an overlay, ActualText/Alt, JS OpenAction, encrypted output, corrupt flate, unknown filter, decode bomb)
and 9 safe outputs (clean page, PyMuPDF `apply_redactions` on text, image pixels and line art, untouched public image, erased rotated/mirrored image, opaque-erased SMask image, cloned shared image).
NEW-20 adds leaky: rotated+scaled and mirrored+/Rotate image, SMask hiding secret colours, SMask carrying a pattern, shared image not cloned (two forms), image in a Form, vector under overlay (rect, stroked line, in a Form). The rotated/mirrored pairs take their region from a RENDER of the page, so a wrong placement mapping fails the leaky or the safe side.
Each leaky output must produce its expected codes; a case that should be caught only by the object scan asserts the extractors stay silent.

## Limits (be honest in receipts)
- A pass is evidence for the listed checks on the given forbidden strings and rects, not a privacy certificate.
- Forbidden strings must be given; the oracle cannot find secrets it is not told about. Glyph-only evidence (outlined text, embedded
  glyph programs, ToUnicode of fonts still in the file) is not inspected beyond the byte/decoded scans.
- CID/subset text is covered only through the extractors (strings are glyph codes, not Unicode).
- Image region check: XObject images only (inline images are UNSUPPORTED, a failure). 90 degree turns and flips test every pixel square touching the region; arbitrary angles test pixel centres inside the region (a sliver of edge pixels is not checked). Colour-key `/Mask` arrays are covered by the colour check only; `SMaskInData` is UNSUPPORTED. Samples are read at the image's own resolution, so a clipped/transparent-by-alpha occurrence is still judged on its samples (deliberate).
- Vector check: geometry only. Clips, blend/opacity, shadings (`sh`) and tiling patterns are not evaluated; a path hidden by a clip or 0 opacity still counts. Non-axis-aligned fills are polygons (curves sampled at 12 steps).
- Raster export outputs: use the image-region check on the page image; text checks pass vacuously by construction.
- Encrypted outputs fail by design (the engine never re-encrypts); AES inputs need `cryptography` for pypdf, otherwise TEXT_PYPDF fails loudly.
