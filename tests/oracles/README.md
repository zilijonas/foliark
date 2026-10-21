# Independent PDF oracles (NEW-7)

The Foliark engine never grades itself. These checks read an OUTPUT PDF with three unrelated parsers
(pypdf, pikepdf/qpdf, PyMuPDF/MuPDF) and know nothing about `foliark-*` crates. Dev-only: none of this ships.

## Setup and run
```sh
python3 -m venv tests/oracles/.venv && tests/oracles/.venv/bin/pip install --only-binary=:all: -r tests/oracles/requirements.txt
tests/oracles/.venv/bin/python tests/oracles/controls.py          # self-test; exit 0 only if every control behaves
tests/oracles/.venv/bin/python tests/oracles/objects.py out.pdf --forbid SECRET_A --forbid SECRET_B [--allow metadata] [--password pw]
tests/oracles/.venv/bin/python tests/oracles/objects.py out.pdf --spec spec.json   # adds "image_regions", "cap"
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
| Image pixels | every decoded sample of an image under a display-space rect is one colour (optionally an exact `color`) | IMAGE_REGION |
| Never silent | decode error, truncated flate, unknown filter, decode cap, unreadable file, extractor failure, crash, timeout | DECODE, UNSUPPORTED, CAP, OPEN, TIMEOUT, CRASH |

`controls.py` builds 14 leaky outputs (overlay over live text, Info, XMP, attachment, prior revision, secret only in an unused Form,
hidden OCG, image pixels under an overlay, ActualText/Alt, JS OpenAction, encrypted output, corrupt flate, unknown filter, decode bomb)
and 4 safe outputs (clean page, PyMuPDF `apply_redactions` on text and on image pixels, untouched public image).
Each leaky output must produce its expected codes; a case that should be caught only by the object scan asserts the extractors stay silent.

## Limits (be honest in receipts)
- A pass is evidence for the listed checks on the given forbidden strings and rects, not a privacy certificate.
- Forbidden strings must be given; the oracle cannot find secrets it is not told about. Glyph-only evidence (outlined text, embedded
  glyph programs, ToUnicode of fonts still in the file) is not inspected beyond the byte/decoded scans.
- CID/subset text is covered only through the extractors (strings are glyph codes, not Unicode).
- Image region check: pages with rotation 0, upright axis-aligned images, XObject images only; images with SMask/Mask, rotated or flipped
  transforms and inline images are reported as UNSUPPORTED (a failure), never skipped.
- Raster export outputs: use the image-region check on the page image; text checks pass vacuously by construction.
- Encrypted outputs fail by design (the engine never re-encrypts); AES inputs need `cryptography` for pypdf, otherwise TEXT_PYPDF fails loudly.
