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
import fidelity  # noqa: E402
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


def leak_dangling():
    return simple(text(20, 360, "KEEP_PUBLIC_TEXT"), {6: b"<< /Ref 99 0 R >>"}, resources_extra="/Properties << /P 6 0 R >>")


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


# ---- NEW-20: image placements, masks, shared images, vectors, Forms ----
GREEN = (10, 200, 30)


def noise(w, h, seed=1):
    return bytes((x * 7 + y * 13 + c * 50 + seed * 31) % 251 for y in range(h) for x in range(w) for c in range(3))


def half_pixmap():
    """64x32: left half one colour (GREEN = 'erased'), right half noise ('secret')."""
    rows = b"".join(bytes(GREEN) * 32 + noise(32, 1, y)[:96] for y in range(32))
    return fitz.Pixmap(fitz.csRGB, 64, 32, rows, False)


def flip_x(doc, pg):
    """Mirror the last image placement horizontally by editing its cm (PyMuPDF cannot insert mirrored images)."""
    xr = pg.get_contents()[0]
    c = doc.xref_stream(xr).decode("latin-1")
    import re
    m = list(re.finditer(r"(\S+) (\S+) (\S+) (\S+) (\S+) (\S+) cm\s+/\S+ Do", c))[-1]
    a, b, cc, d, e, f = (float(v) for v in m.groups())
    c = c[:m.start()] + "%g %g %g %g %g %g cm\n/%s" % (-a, b, cc, d, e + a, f, m.group(0).rsplit("/", 1)[1]) + c[m.end():]
    doc.update_stream(xr, c.encode("latin-1"))


def zones(pg):
    """Display-space (uniform_rect, secret_rect) of a half_pixmap placement, found by RENDERING (ground truth independent of the oracle's maths)."""
    pm = pg.get_pixmap(dpi=72)
    a, n = [], []
    for y in range(pm.height):
        for x in range(pm.width):
            c = pm.pixel(x, y)
            if c == (255, 255, 255):
                continue
            (a if max(abs(c[i] - GREEN[i]) for i in range(3)) < 6 else n).append((x, y))
    box = lambda pts: [min(p[0] for p in pts) + 3, min(p[1] for p in pts) + 3, max(p[0] for p in pts) - 2, max(p[1] for p in pts) - 2]
    return box(a), box(n)


def placed(rotate=0, flip=False, page_rot=0, overlay_secret=False, which=0):
    d = fitz.open()
    pg = d.new_page(width=300, height=400)
    pg.insert_image(fitz.Rect(40, 100, 200, 180), pixmap=half_pixmap(), rotate=rotate)
    if flip:
        flip_x(d, pg)
    pg.set_rotation(page_rot)
    rects = zones(pg)
    if overlay_secret:
        pg.draw_rect(fitz.Rect(*rects[1]) * ~pg.rotation_matrix, color=(0, 0, 0), fill=(0, 0, 0))
    return d, rects[which]


def img_case(which, **kw):
    """-> (builder, spec). which=0: region over the erased half (safe), 1: over the secret half (leaky)."""
    d, rect = placed(which=which, **kw)
    return (lambda: out(d)), {"image_regions": [{"page": 0, "rect": rect}]}


def masked(color_png, mask_png):
    d = fitz.open()
    pg = d.new_page(width=300, height=400)
    pg.insert_image(fitz.Rect(20, 50, 148, 178), stream=color_png, mask=mask_png)
    return out(d)


def gray(v):
    return fitz.Pixmap(fitz.csGRAY, 64, 64, bytes(v(x, y) for y in range(64) for x in range(64)), False).tobytes("png")


def rgb(buf):
    return fitz.Pixmap(fitz.csRGB, 64, 64, buf, False).tobytes("png")


def shared(clone0):
    """Two pages show one image. clone0: page 0 gets an erased clone, page 1 keeps the original xref. Else page 0 only gets a black overlay."""
    d = fitz.open()
    for _ in range(2):
        d.new_page(width=300, height=400)
    p0, p1 = d[0], d[1]  # page objects from new_page lose their parent when a second page is added
    pm = fitz.Pixmap(fitz.csRGB, 64, 64, noise(64, 64), False)
    r = fitz.Rect(20, 50, 148, 178)
    if clone0:
        p0.insert_image(r, pixmap=fitz.Pixmap(fitz.csRGB, 64, 64, bytes(64 * 64 * 3), False))
        p1.insert_image(r, pixmap=pm)
    else:
        p1.insert_image(r, pixmap=pm)
        p0.insert_image(r, xref=p1.get_images()[0][0])
        p0.draw_rect(fitz.Rect(*IMG_RECT), color=(0, 0, 0), fill=(0, 0, 0))
    return out(d)


SHAPE = fitz.Rect(60, 200, 120, 240)
VEC_RECT = [50, 190, 140, 250]


def vec_page(diag=False):
    d = fitz.open()
    pg = d.new_page(width=300, height=400)
    pg.insert_text((20, 360), "KEEP_PUBLIC_TEXT", fontsize=14)
    pg.draw_rect(fitz.Rect(200, 300, 250, 340), color=None, fill=(0, 0, 1))  # public shape elsewhere
    if diag:
        pg.draw_line((30, 180), (110, 215), color=(1, 0, 0), width=3)  # stroked line entering the region
    else:
        pg.draw_rect(SHAPE, color=(1, 0, 0), fill=(1, 0, 0))
        pg.draw_circle((90, 220), 10, color=(0, 0.5, 0), fill=(0, 1, 0))
    return d, pg


def leak_vec(diag=False):
    d, pg = vec_page(diag)
    pg.draw_rect(fitz.Rect(*VEC_RECT), color=(0, 0, 0), fill=(0, 0, 0))
    return out(d)


def safe_vec():
    d, pg = vec_page()
    redact(d, pg, VEC_RECT)  # PyMuPDF removes fully covered line art, then paints the fill
    return out(d)


def in_form(kind):
    src = fitz.open()
    sp = src.new_page(width=300, height=400)
    if kind == "image":
        sp.insert_image(fitz.Rect(20, 50, 148, 178), pixmap=fitz.Pixmap(fitz.csRGB, 64, 64, noise(64, 64), False))
    else:
        sp.draw_rect(fitz.Rect(60, 200, 120, 240), color=(1, 0, 0), fill=(1, 0, 0))
    d = fitz.open()
    pg = d.new_page(width=300, height=400)
    pg.show_pdf_page(pg.rect, src, 0)
    pg.draw_rect(fitz.Rect(*(IMG_RECT if kind == "image" else VEC_RECT)), color=(0, 0, 0), fill=(0, 0, 0))
    return out(d)


NOISE_PNG = rgb(noise(64, 64))
BLACK_PNG = rgb(bytes(64 * 64 * 3))
VEC = {"vector_regions": [{"page": 0, "rect": VEC_RECT, "fill": [0, 0, 0]}]}
SHARED2 = {"image_regions": [{"page": 0, "rect": IMG_RECT}, {"page": 1, "rect": IMG_RECT}]}


# ---- NEW-21: fidelity pairs (input, output). Region = the secret line; everything else must be unchanged ----
def fpage(keep_x=20, image=True, secret=True, keep_as_image=False):
    d = fitz.open()
    pg = d.new_page(width=300, height=400)
    if keep_as_image:  # same look, no text layer
        src, sp = fitz.open(), None
        sp = src.new_page(width=300, height=400)
        sp.insert_text((keep_x, 360), "KEEP_PUBLIC_TEXT", fontsize=14)
        clip = fitz.Rect(10, 340, 160, 370)
        pg.insert_image(clip, pixmap=sp.get_pixmap(dpi=72, clip=clip))
    else:
        pg.insert_text((keep_x, 360), "KEEP_PUBLIC_TEXT", fontsize=14)
    pg.insert_text((20, 330), "second public line stays", fontsize=12)
    if secret:
        pg.insert_text((20, 300), SECRET, fontsize=14)
    if image:
        px = bytes((x * 4 + y * 2 + c * 50) % 256 for y in range(64) for x in range(64) for c in range(3))
        pg.insert_image(fitz.Rect(20, 50, 148, 178), pixmap=fitz.Pixmap(fitz.csRGB, 64, 64, px, False))
        pg.draw_rect(fitz.Rect(200, 60, 260, 120), color=(0, 0, 1), fill=(0.8, 0.8, 1))
    return d, pg


def fin():
    d, pg = fpage()
    return d.tobytes(), list(pg.search_for(SECRET)[0])


def fout_redacted(**kw):
    d, pg = fpage(**kw)
    redact(d, pg, pg.search_for(SECRET)[0])
    return out(d)


def fout_raster(declared_text=False):
    src = fitz.open(stream=fout_redacted(), filetype="pdf")
    d = fitz.open()
    pg = d.new_page(width=300, height=400)
    pg.insert_image(pg.rect, pixmap=src[0].get_pixmap(dpi=144))
    return out(d)


def fout_blank_secret_image_only():
    d, pg = fpage(secret=False)
    return out(d)


FIN, REGION = fin()
FSPEC = {"changed": [{"page": 0, "rect": REGION}], "preserve": ["KEEP_PUBLIC_TEXT", "secondpublicline"]}
RASTER = {**FSPEC, "raster": True, "preserve": [], "threshold": 64, "max_frac": 0.01}
FCASES = [
    ("fail: unaffected image dropped", lambda: fout_redacted(image=False), FSPEC, {"PIXELS"}, set()),
    ("fail: unaffected text shifted 5 pt", lambda: fout_redacted(keep_x=25), FSPEC, {"PIXELS", "WORDS_FITZ", "WORDS_PYPDF"}, set()),
    ("fail: unaffected text lost (outlined as image)", lambda: fout_redacted(keep_as_image=True), FSPEC, {"WORDS_FITZ", "WORDS_PYPDF", "PRESERVE_FITZ", "PRESERVE_PYPDF"}, {"PIXELS"}),  # pixel-identical: only the text checks can see it
    ("fail: raster output not declared as raster", fout_raster, FSPEC, {"WORDS_FITZ", "WORDS_PYPDF", "PRESERVE_FITZ", "PRESERVE_PYPDF"}, set()),
    ("fail: declared raster still has a text layer", fout_redacted, RASTER, {"RASTER_TEXT"}, set()),
    ("pass: PyMuPDF apply_redactions", fout_redacted, FSPEC, None, set()),
    ("pass: declared raster, no text layer", fout_raster, RASTER, None, set()),
]


S = {"forbidden": [SECRET]}
IMG = {"image_regions": [{"page": 0, "rect": IMG_RECT}]}
# name, builder, spec, codes that MUST appear (leaky) / None (safe), codes that must NOT appear[, substring a finding must contain]
CASES = [
    ("leaky: black rect over live text", leak_overlay, S, {"TEXT_PYPDF", "TEXT_FITZ", "STREAM"}, set()),
    ("leaky: /Info title", leak_info, {"forbidden": ["SECRET_META_TITLE"]}, {"FEATURE", "STRING"}, set()),
    ("leaky: XMP packet", leak_xmp, {"forbidden": ["SECRET_XMP_BODY"]}, {"FEATURE", "STREAM"}, set()),
    ("leaky: embedded attachment", leak_attachment, {"forbidden": ["SECRET_ATT_BODY", "SECRET_ATT_NAME"]}, {"FEATURE", "STREAM"}, set()),
    ("leaky: prior revision with secret", gen.incremental_prior_text, {"forbidden": ["SECRET_OLD_REVISION"]}, {"REVISION", "BYTES_RAW"}, set()),
    ("leaky: secret only in unused Form XObject", leak_unused_form, {"forbidden": ["SECRET_UNUSED_FORM"]}, {"STREAM"}, {"TEXT_PYPDF", "TEXT_FITZ"}),
    ("leaky: secret in hidden OCG", gen.ocg_hidden, {"forbidden": ["SECRET_HIDDEN_OCG"]}, {"STREAM", "FEATURE"}, set()),
    ("leaky: image pixels under overlay", leak_image_under_overlay, IMG, {"IMAGE_REGION"}, set()),
    # NEW-100: glyphs only their font program's cmap names (iOS Quartz Type0, ToUnicode maps nothing); both extractors miss them.
    ("leaky: secret in a font whose ToUnicode maps nothing", gen.quartz_type0_empty_tounicode, {"forbidden": [gen.QUARTZ_SECRET]}, {"TEXT_GLYPH"}, {"TEXT_FITZ"}),
    ("leaky: secret word carrying zero-width combining marks", gen.quartz_type0_combining_marks, {"forbidden": [gen.MARKS_SECRET]}, {"TEXT_GLYPH"}, {"TEXT_FITZ"}),
    # NEW-104: /W wider than the drawn advances (TJ corrections), an ogonek as a zero-advance mark.
    ("leaky: word in a font whose /W is wider than it draws", gen.quartz_type0_tight_widths, {"forbidden": [gen.TIGHT_WORD]}, {"TEXT_GLYPH"}, {"TEXT_FITZ"}),
    # NEW-106: Word's bold heading directly above its regular body; the body word an edit replaces.
    ("leaky: body word under a bold Word heading", gen.word_bold_heading_body, {"forbidden": [gen.STYLE_SECRET]}, {"TEXT_FITZ", "TEXT_PYPDF"}, set()),
    ("leaky: ActualText / Alt", leak_actualtext, {"forbidden": ["SECRET_ACTUALTEXT", "SECRET_ALT_TEXT"]}, {"STREAM"}, set()),
    ("leaky: JS OpenAction + metadata + attachment", gen.leaks_metadata_attachment_js, {"forbidden": ["SECRET_OPENACTION_JS", "SECRET_ATTACHMENT_BODY"]}, {"FEATURE", "STRING", "STREAM"}, set()),
    ("leaky: encrypted output", lambda: gen._encrypted("AES_128"), {"password": gen.ENC_USER}, {"FEATURE"}, set()),
    ("leaky: reference to an undefined object", leak_dangling, {}, {"DANGLING"}, set()),
    ("never silent: corrupt flate", gen.corrupt_flate, {}, {"DECODE"}, set()),
    ("never silent: unsupported filter", leak_bogus_filter, {}, {"UNSUPPORTED"}, set()),
    ("never silent: decode cap", leak_bomb, {"cap": 1_000_000}, {"CAP"}, set()),
    ("safe: public-only page", safe_clean, S, None, set()),
    ("safe: Quartz font page, secret not shown", gen.quartz_type0_empty_tounicode, {"forbidden": ["SECRET_NOT_SHOWN"]}, None, set()),
    ("safe: combining-mark page, secret not shown", gen.quartz_type0_combining_marks, {"forbidden": ["SECRET_NOT_SHOWN"]}, None, set()),
    ("safe: tight-width Quartz page, secret not shown", gen.quartz_type0_tight_widths, {"forbidden": ["SECRET_NOT_SHOWN"]}, None, set()),
    ("safe: bold-heading Word page, secret not shown", gen.word_bold_heading_body, {"forbidden": ["SECRET_NOT_SHOWN"]}, None, set()),
    ("safe: apply_redactions on text", safe_redact_text, S, None, set()),
    ("safe: apply_redactions image pixels", safe_redact_image, IMG, None, set()),
    ("safe: untouched public image, no region", safe_untouched_image, S, None, set()),
    # NEW-20. Placement cases take their region from a render, so a wrong CTM mapping fails one side of each pair.
    ("leaky: rot90 scaled image, secret half under overlay", *img_case(1, rotate=90, overlay_secret=True), {"IMAGE_REGION"}, {"UNSUPPORTED"}),
    ("safe: rot90 scaled image, region on erased half", *img_case(0, rotate=90), None, set()),
    ("leaky: mirrored image on /Rotate 90 page, secret half", *img_case(1, flip=True, page_rot=90, overlay_secret=True), {"IMAGE_REGION"}, {"UNSUPPORTED"}),
    ("safe: mirrored image on /Rotate 90 page, erased half", *img_case(0, flip=True, page_rot=90), None, set()),
    ("leaky: SMask hides secret colour data (unmask reveals)", lambda: masked(NOISE_PNG, gray(lambda x, y: 0)), IMG, {"IMAGE_REGION"}, {"UNSUPPORTED"}, "colour"),
    ("leaky: colour erased but SMask carries a pattern", lambda: masked(BLACK_PNG, gray(lambda x, y: (x * 4) % 256)), IMG, {"IMAGE_REGION"}, {"UNSUPPORTED"}, "SMask"),
    ("safe: colour black and SMask opaque in region", lambda: masked(BLACK_PNG, gray(lambda x, y: 255)), IMG, None, set()),
    ("leaky: shared image, page 0 overlay only (not cloned)", lambda: shared(False), IMG, {"IMAGE_REGION"}, set(), "shared, also used on pages [1]"),
    ("leaky: shared image, page 1 occurrence left intact", lambda: shared(True), SHARED2, {"IMAGE_REGION"}, set(), "page 1 occurrence"),
    ("safe: shared image, page 0 cloned+erased, page 1 not declared", lambda: shared(True), IMG, None, set()),
    ("leaky: image inside Form XObject under overlay", lambda: in_form("image"), IMG, {"IMAGE_REGION"}, set()),
    ("leaky: vector shape under black overlay", leak_vec, VEC, {"VECTOR_REGION"}, set(), "path #"),
    ("leaky: stroked line under black overlay", lambda: leak_vec(True), VEC, {"VECTOR_REGION"}, set()),
    ("leaky: vector inside Form XObject under overlay", lambda: in_form("vector"), VEC, {"VECTOR_REGION"}, set()),
    ("safe: vector removed, only declared black fill left", safe_vec, VEC, None, set()),
]


def main():
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="foliark-controls-"))
    bad, leaky, caught, safe, passed = 0, 0, 0, 0, 0
    for i, (name, build, spec, must, mustnot, *needle) in enumerate(CASES):
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
            ok = must <= codes and not (mustnot & codes) and all(any(n in f for f in codes_full) for n in needle)
            caught += ok
        bad += not ok
        print("%s %-46s %s" % ("ok  " if ok else "FAIL", name, sorted(codes) if codes else "clean"))
        if not ok:
            print("     findings:", *codes_full[:6], sep="\n       ")
    print("leaky caught %d/%d, safe passed %d/%d" % (caught, leaky, passed, safe))
    fbad, ffail, fcaught, fpass, fok = 0, 0, 0, 0, 0
    (tmp / "in.pdf").write_bytes(FIN)
    for i, (name, build, spec, must, mustnot) in enumerate(FCASES):
        p = tmp / ("f%02d.pdf" % i)
        p.write_bytes(build())
        findings, notes = fidelity.check_isolated(tmp / "in.pdf", p, spec)
        codes = {f.split(":")[0] for f in findings}
        if must is None:
            fpass += 1
            ok = not findings
            fok += ok
        else:
            ffail += 1
            ok = must <= codes and not (mustnot & codes)
            fcaught += ok
        fbad += not ok
        print("%s %-46s %s" % ("ok  " if ok else "FAIL", name, sorted(codes) if codes else "clean"))
        if not ok:
            print("     findings:", *(findings[:6] + notes), sep="\n       ")
    print("fidelity fail cases caught %d/%d, pass cases passed %d/%d" % (fcaught, ffail, fok, fpass))
    return 1 if bad or fbad else 0


if __name__ == "__main__":
    sys.exit(main())
