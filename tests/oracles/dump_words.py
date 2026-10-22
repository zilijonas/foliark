#!/usr/bin/env python3
"""Dump PyMuPDF `get_text("words", sort=True)` (small glyph heights on) per page into
crates/foliark-engine/tests/data/pymupdf-words/<fixture>.json. Dev-only oracle for the Rust
parity test (crates/foliark-engine/tests/text_mapping.rs); the JSON is committed, this script
is the only way it is produced. Boxes are mapped into display space with `rotation_matrix`
(spec section 2). Usage: python3 tests/oracles/dump_words.py [fixture ...] (paths relative to
tests/fixtures, default FIXTURES)."""
import json
import pathlib
import sys

import fitz

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/foliark-engine/tests/data/pymupdf-words"
# Native-text fixtures the strict reader opens (encrypted, repaired, truncated, corrupt and
# decode-ratio and image-size bombs, cyclic and 40-deep Form files are refused by design and have no parity row).
FIXTURES = [
    "generated/acroform-values.pdf", "generated/annot-appearance.pdf",
    "generated/form-reuse-matrices.pdf", 
    "generated/incremental-prior-text.pdf", "generated/leaks-metadata-attachment-js.pdf",
    "generated/ocg-hidden-text.pdf", "generated/ocr-over-image.pdf",
    "hddn/acroform.pdf", "hddn/actions-js.pdf", "hddn/actualtext.pdf",
    "hddn/annotation-comment.pdf", "hddn/attachment.pdf", 
    "hddn/hidden-ocr-text.pdf", "hddn/incremental-revision.pdf", "hddn/metadata-xmp.pdf",
    "hddn/optional-content-layer.pdf", "hddn/outline-bookmark.pdf",
    "hddn/rotation-crop.pdf", "hddn/shared-image.pdf", "hddn/shared-vector.pdf",
    "hddn/thumbnail.pdf", "hddn/unicode-glyph-widths.pdf",
]


def dump(rel: str) -> None:
    doc = fitz.open(ROOT / "tests/fixtures" / rel)
    pages = []
    for page in doc:
        m = page.rotation_matrix
        words = []
        for x0, y0, x1, y1, text, block, line, word in page.get_text("words", sort=True):
            r = fitz.Rect(x0, y0, x1, y1) * m
            words.append({"box": [r.x0, r.y0, r.x1, r.y1], "text": text,
                          "block": block, "line": line, "word": word})
        pages.append({"width": page.rect.width, "height": page.rect.height, "words": words})
    name = rel.replace("/", "__").removesuffix(".pdf") + ".json"
    (OUT / name).write_text(json.dumps({"fixture": rel, "pages": pages}, indent=1, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    fitz.TOOLS.set_small_glyph_heights(True)
    OUT.mkdir(parents=True, exist_ok=True)
    for f in sys.argv[1:] or FIXTURES:
        dump(f)
    print(f"{fitz.__doc__.split(':')[0]}: wrote {len(sys.argv[1:] or FIXTURES)} files to {OUT.relative_to(ROOT)}")
