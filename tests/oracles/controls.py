#!/usr/bin/env python3
"""Negative/positive controls for objects.py (NEW-7). Exit 0 iff every LEAKY output FAILS the oracle with the expected
finding code(s) and every SAFE output PASSES. Run before trusting the oracle on any real output:
  tests/oracles/.venv/bin/python tests/oracles/controls.py
"""
import pathlib
import sys
import tempfile

import fitz

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "fixtures" / "generators"))
import generate as gen  # noqa: E402
import objects  # noqa: E402
from pdfsyn import simple, stream, text  # noqa: E402

SECRET = "SECRET_TEXT_XYZ"
IMG_RECT = [40, 70, 100, 110]  # display-space region inside the 128x128 pt image at (20,50)


def page(img=False, secret=True):
    doc = fitz.open()
    pg = doc.new_page(width=300, height=400)
    pg.insert_text((20, 360), "KEEP_PUBLIC_TEXT", fontsize=14)
    if secret:
        pg.insert_text((20, 300), SECRET, fontsize=14)
    if img:
        px = bytes((x * 4 + y * 2 + c * 50) % 256 for y in range(64) for x in range(64) for c in range(3))
        pg.insert_image(fitz.Rect(20, 50, 148, 178), pixmap=fitz.Pixmap(fitz.csRGB, 64, 64, px, False))
    return doc, pg


def out(doc):
    doc.set_metadata({})
    doc.del_xml_metadata()
    return doc.tobytes(garbage=4, deflate=True)


def redact(doc, pg, rect, images=fitz.PDF_REDACT_IMAGE_NONE):
    pg.add_redact_annot(fitz.Rect(rect), fill=(0, 0, 0))
    pg.apply_redactions(images=images)


def leak_overlay():
    d, p = page()
    p.draw_rect(fitz.Rect(15, 285, 200, 305), color=(0, 0, 0), fill=(0, 0, 0))
    return out(d)


def leak_info():
    d, _ = page(secret=False)
    d.set_metadata({"title": "SECRET_META_TITLE"})
    return d.tobytes(garbage=4, deflate=True)


def leak_xmp():
    d, _ = page(secret=False)
    d.set_metadata({})
    d.set_xml_metadata("<x:xmpmeta xmlns:x='adobe:ns:meta/'>SECRET_XMP_BODY</x:xmpmeta>")
    return d.tobytes(garbage=4, deflate=True)


def leak_attachment():
    d, _ = page(secret=False)
    d.embfile_add("SECRET_ATT_NAME.txt", b"SECRET_ATT_BODY")
    return out_keep(d)


def out_keep(d):
    d.set_metadata({})
    return d.tobytes(garbage=4, deflate=True)


def leak_unused_form():
    f = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Resources << /Font << /F1 5 0 R >> >>", text(20, 200, "SECRET_UNUSED_FORM"), True)
    return simple(text(20, 360, "KEEP_PUBLIC_TEXT"), {6: f}, resources_extra="/XObject << /Fm 6 0 R >>")


def leak_actualtext():
    c = b"/Span << /ActualText (SECRET_ACTUALTEXT) >> BDC\n" + text(20, 360, "KEEP_PUBLIC_TEXT") + b"EMC\n/Figure << /Alt (SECRET_ALT_TEXT) >> BDC EMC\n"
    return simple(c, flate=True)


def leak_image_under_overlay():
    d, p = page(img=True, secret=False)
    p.draw_rect(fitz.Rect(*IMG_RECT), color=(0, 0, 0), fill=(0, 0, 0))
    return out(d)


def leak_bogus_filter():
    return simple(b"", extra={4: stream("/Filter /Bogus", b"abc")})


def leak_bomb():
    return simple(b"", extra={4: stream("", b"A" * 5_000_000, True)})


def safe_clean():
    d, _ = page(secret=False)
    return out(d)


def safe_redact_text():
    d, p = page()
    redact(d, p, p.search_for(SECRET)[0])
    return out(d)


def safe_redact_image():
    d, p = page(img=True, secret=False)
    redact(d, p, IMG_RECT, fitz.PDF_REDACT_IMAGE_PIXELS)
    return out(d)


def safe_untouched_image():
    d, _ = page(img=True, secret=False)
    return out(d)


S = {"forbidden": [SECRET]}
IMG = {"image_regions": [{"page": 0, "rect": IMG_RECT}]}
# name, builder, spec, codes that MUST appear (leaky) / None (safe), codes that must NOT appear
CASES = [
    ("leaky: black rect over live text", leak_overlay, S, {"TEXT_PYPDF", "TEXT_FITZ", "STREAM"}, set()),
    ("leaky: /Info title", leak_info, {"forbidden": ["SECRET_META_TITLE"]}, {"FEATURE", "STRING"}, set()),
    ("leaky: XMP packet", leak_xmp, {"forbidden": ["SECRET_XMP_BODY"]}, {"FEATURE", "STREAM"}, set()),
    ("leaky: embedded attachment", leak_attachment, {"forbidden": ["SECRET_ATT_BODY", "SECRET_ATT_NAME"]}, {"FEATURE", "STREAM"}, set()),
    ("leaky: prior revision with secret", gen.incremental_prior_text, {"forbidden": ["SECRET_OLD_REVISION"]}, {"REVISION", "BYTES_RAW"}, set()),
    ("leaky: secret only in unused Form XObject", leak_unused_form, {"forbidden": ["SECRET_UNUSED_FORM"]}, {"STREAM"}, {"TEXT_PYPDF", "TEXT_FITZ"}),
    ("leaky: secret in hidden OCG", gen.ocg_hidden, {"forbidden": ["SECRET_HIDDEN_OCG"]}, {"STREAM", "FEATURE"}, set()),
    ("leaky: image pixels under overlay", leak_image_under_overlay, IMG, {"IMAGE_REGION"}, set()),
    ("leaky: ActualText / Alt", leak_actualtext, {"forbidden": ["SECRET_ACTUALTEXT", "SECRET_ALT_TEXT"]}, {"STREAM"}, set()),
    ("leaky: JS OpenAction + metadata + attachment", gen.leaks_metadata_attachment_js, {"forbidden": ["SECRET_OPENACTION_JS", "SECRET_ATTACHMENT_BODY"]}, {"FEATURE", "STRING", "STREAM"}, set()),
    ("leaky: encrypted output", lambda: gen._encrypted("AES_128"), {"password": gen.ENC_USER}, {"FEATURE"}, set()),
    ("never silent: corrupt flate", gen.corrupt_flate, {}, {"DECODE"}, set()),
    ("never silent: unsupported filter", leak_bogus_filter, {}, {"UNSUPPORTED"}, set()),
    ("never silent: decode cap", leak_bomb, {"cap": 1_000_000}, {"CAP"}, set()),
    ("safe: public-only page", safe_clean, S, None, set()),
    ("safe: apply_redactions on text", safe_redact_text, S, None, set()),
    ("safe: apply_redactions image pixels", safe_redact_image, IMG, None, set()),
    ("safe: untouched public image, no region", safe_untouched_image, S, None, set()),
]


def main():
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="foliark-controls-"))
    bad, leaky, caught, safe, passed = 0, 0, 0, 0, 0
    for i, (name, build, spec, must, mustnot) in enumerate(CASES):
        p = tmp / ("c%02d.pdf" % i)
        p.write_bytes(build())
        codes_full = objects.check_isolated(p, spec)
        codes = {f.split(":")[0] for f in codes_full}
        if must is None:
            safe += 1
            ok = not codes_full
            passed += ok
        else:
            leaky += 1
            ok = must <= codes and not (mustnot & codes)
            caught += ok
        bad += not ok
        print("%s %-46s %s" % ("ok  " if ok else "FAIL", name, sorted(codes) if codes else "clean"))
        if not ok:
            print("     findings:", *codes_full[:6], sep="\n       ")
    print("leaky caught %d/%d, safe passed %d/%d" % (caught, leaky, passed, safe))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
