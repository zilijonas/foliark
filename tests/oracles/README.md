# Independent PDF oracles (NEW-7, NEW-20, NEW-21)

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
| Image pixels | every image occurrence (page, Form XObject, any CTM: rotated, scaled, mirrored, any page /Rotate) under a display-space rect has one colour in its colour samples AND in its SMask / `/Mask` samples (an `/ImageMask` stencil image is unpacked bit by bit, NEW-35) (optionally an exact `color`); one finding per occurrence, naming the other pages that share the xref (a surviving occurrence still holding the pixels = missing clone) | IMAGE_REGION |
| Vector paths | no filled or stroked path geometry (page or Form content, PyMuPDF `get_drawings`) reaches into a display-space rect beyond `tol` (0.25 pt) except the declared `fill` (RGB 0-255, must be the last painted hit and cover the region) and paths inside a declared `allow` rect | VECTOR_REGION |
| Occurrences | the Form at `path` (n-th Form drawn by the page, then n-th drawn by that): no `forbid` string in anything it reaches (nested Forms, decoded + shown text), every `require` string present; with `no_unused_xobjects` every XObject resource of a page or drawn Form is drawn by its own content (an original nobody draws must not stay reachable) (NEW-38) | OCCURRENCE, UNUSED_XOBJECT |
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

## Image samples for tests (NEW-34/35, `samples.py`)
`samples.py out.pdf` prints every page's image XObjects and every image object of the file as JSON (pikepdf only: decoded sample hex, color space, bits, filter, SMask, /Mask, /Decode, page content text); `samples.py --jpeg|--rgba file` prints PIL's decode of a source image. The Rust redaction tests compare these independent samples with geometry computed in the test.

## Fidelity: output vs input outside the changed regions (NEW-21, `fidelity.py`)
```sh
tests/oracles/.venv/bin/python tests/oracles/fidelity.py in.pdf out.pdf --changed 0:x0,y0,x1,y1 [--preserve TEXT] [--raster] [--spec spec.json]
```
`fidelity.check_isolated(in, out, spec)` returns `(findings, notes)`; notes carry per-page metrics (max diff, pixels over threshold) and the raster declaration. Changed rects are display space (spec section 2).
| Check | Proves | Code |
|---|---|---|
| Pixel diff | both PDFs rendered by PyMuPDF at `dpi`; declared regions (+`margin`) masked; pixels differing by more than `threshold` (max channel) stay under `max_frac` of the unmasked area. Catches dropped images/content, shifted text | PIXELS |
| Words, PyMuPDF | every input word outside changed regions has an output word with equal text and bbox within `text_tol`, and every output word outside them has an input word (both directions, greedy 1:1) | WORDS_FITZ |
| Text chunks, pypdf | every pypdf text-show chunk of an input line that does not touch a region has an output chunk with equal text and start point within `text_tol` | WORDS_PYPDF |
| Preserve | each `preserve` string is extractable (whitespace-insensitive) by PyMuPDF and by pypdf | PRESERVE_FITZ, PRESERVE_PYPDF |
| Raster | with `"raster": true`: no words, no pypdf text, no fonts on any page (else RASTER_TEXT); text mapping and preserve are skipped and the loss is printed as a note. Raster must be declared: an undeclared raster output fails WORDS_*/PRESERVE_* | RASTER_TEXT |
| Structure | page count, page size and render size equal | PAGES, SIZE |

Defaults (override in the spec): `dpi` 72, `margin` 2 pt, `threshold` 32 (of 255), `max_frac` 0.0002 (0.02% of unmasked pixels), `text_tol` 1.0 pt.
Raster outputs re-sample every page, so declare looser pixel limits with them (controls use `threshold` 64, `max_frac` 0.01; measured 0.16%).
Controls (in `controls.py`, 5 fail + 2 pass): unaffected image dropped, unaffected text shifted 5 pt, unaffected text lost while pixel-identical (outlined as an image; PIXELS must stay silent),
undeclared raster, declared raster that kept its text layer; pass: PyMuPDF `apply_redactions`, declared raster without text. Measured baselines: identical re-save 0 px, correct redaction 0 px, 5 pt shift 0.8%, dropped image 17%.

Limits: the margin must cover anti-aliasing around changed regions; a region declared too large hides regressions inside it. Pixel diff compares one renderer (MuPDF) on both sides, so it
sees changes, not rendering errors common to both. pypdf chunk matching is operator-granular: an engine that re-splits an unaffected line into different text-show operators fails WORDS_PYPDF
even when the page is right (the PyMuPDF word check is the authoritative layout check); its start points ignore Tc/Tw/Tz and text rise. Reflowed text must be inside a declared region. Annotations render per PyMuPDF defaults.
