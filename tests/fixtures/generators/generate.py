#!/usr/bin/env python3
"""Foliark synthetic fixtures beyond HDDN's 21 (NEW-6). Output: tests/fixtures/generated/<id>.pdf

Pinned: Python 3.9, PyMuPDF==1.26.5 (dev-only, used ONLY to encrypt; everything else is hand-written PDF syntax
from pdfsyn.py and byte-deterministic). Encrypted files embed random salts/IVs, so they are written only when
absent (or with --force) and manifest.json (sha256) is the source of truth for them.

  python3 generate.py            write missing/changed files
  python3 generate.py --force    also regenerate the nondeterministic encrypted ones
"""
import pathlib
import sys
import zlib

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from pdfsyn import W, H, build, region, simple, stream, text  # noqa: E402

OUT = pathlib.Path(__file__).resolve().parent.parent / "generated"
FONT_RES = "/Font << /F1 5 0 R >>"
ENC_USER, ENC_OWNER = "user-pw", "owner-pw"


def corrupt_flate():
    z = bytearray(zlib.compress(text(20, 300, "KEEP_PUBLIC_CORRUPT") + text(20, 270, "SECRET_CORRUPT_FLATE"), 9))
    z[len(z) // 2] ^= 0xFF
    z[len(z) // 2 + 1] ^= 0xFF
    del z[-8:]  # drop the tail and adler32
    try:
        zlib.decompress(bytes(z))
        raise SystemExit("corrupt-flate generator produced a valid stream")
    except zlib.error:
        pass
    return simple(b"", extra={4: stream("/Filter /FlateDecode", bytes(z))})


def _form(content, res="", bbox="0 0 300 400", matrix=""):
    return stream("/Type /XObject /Subtype /Form /BBox [%s] %s /Resources << %s %s >>" % (bbox, matrix, FONT_RES, res), content)


def forms_cyclic():
    ex = {6: _form(text(20, 200, "SECRET_CYCLIC_A") + b"/B Do\n", "/XObject << /B 7 0 R >>"),
          7: _form(text(20, 170, "SECRET_CYCLIC_B") + b"/A Do\n", "/XObject << /A 6 0 R >>")}
    return simple(text(20, 360, "KEEP_PUBLIC_CYCLIC") + b"/A Do\n", ex, resources_extra="/XObject << /A 6 0 R >>")


def forms_deep(n=40):  # > spec maxNesting (32)
    ex = {}
    for i in range(n):
        last = i == n - 1
        body = text(20, 200, "SECRET_DEEP_FORM_%d" % n) if last else b"q 1 0 0 1 0 0 cm /N Do Q\n"
        ex[6 + i] = _form(body, "" if last else "/XObject << /N %d 0 R >>" % (7 + i))
    return simple(text(20, 360, "KEEP_PUBLIC_DEEP") + b"/N Do\n", ex, resources_extra="/XObject << /N 6 0 R >>")


def form_reuse_matrices():
    ex = {6: _form(text(4, 4, "SECRET_FORM_REUSE", 12), bbox="0 0 200 30", matrix="/Matrix [1.5 0 0 1.5 0 0]")}
    c = (text(20, 360, "KEEP_PUBLIC_FORM_REUSE") + b"q 1 0 0 1 20 300 cm /Fm Do Q\n" + b"q 0.5 0 0 0.5 20 250 cm /Fm Do Q\n"
         + b"q 0 1 -1 0 250 100 cm /Fm Do Q\n")
    return simple(c, ex, resources_extra="/XObject << /Fm 6 0 R >>")


def many_pages(n=1001):
    objs = {1: "<< /Type /Catalog /Pages 2 0 R >>",
            2: "<< /Type /Pages /Kids [%s] /Count %d >>" % (" ".join("%d 0 R" % (5 + i) for i in range(n)), n),
            3: stream("", text(20, 360, "PAGE_COMMON_TEXT"), True),
            4: "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"}
    for i in range(n):  # one shared content stream keeps the file small; no per-page text markers
        objs[5 + i] = ("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %d %d] /Contents 3 0 R /Resources << /Font << /F1 4 0 R >> >> >>" % (W, H))
    return build(objs)[0]


def ocg_hidden():
    c = (text(20, 360, "KEEP_PUBLIC_OCG") + b"/OC /oc2 BDC\n" + text(20, 320, "KEEP_PUBLIC_ON_LAYER") + b"EMC\n"
         + b"/OC /oc1 BDC\n" + text(20, 280, "SECRET_HIDDEN_OCG") + b"EMC\n")
    ex = {6: "<< /Type /OCG /Name (SECRET_LAYER_OFF_NAME) >>", 7: "<< /Type /OCG /Name (Public layer) >>"}
    cat = "/OCProperties << /OCGs [6 0 R 7 0 R] /D << /Order [6 0 R 7 0 R] /OFF [6 0 R] >> >>"
    return simple(c, ex, cat_extra=cat, resources_extra="/Properties << /oc1 6 0 R /oc2 7 0 R >>")


def _bar_image(secret=b"\0\0\0"):  # 64x32 RGB white; secret black bar cols 8..40 rows 8..16; public gray bar cols 44..60 rows 8..16
    px = bytearray(b"\xff" * (64 * 32 * 3))
    for y in range(8, 16):
        for x in range(8, 40):
            px[(y * 64 + x) * 3:(y * 64 + x) * 3 + 3] = secret
        for x in range(44, 60):
            px[(y * 64 + x) * 3:(y * 64 + x) * 3 + 3] = b"\x80\x80\x80"
    return bytes(px)


def ocr_over_image():
    img = stream("/Type /XObject /Subtype /Image /Width 64 /Height 32 /ColorSpace /DeviceRGB /BitsPerComponent 8", _bar_image(), True)
    c = b"q 200 0 0 100 20 200 cm /Im1 Do Q\n" + text(45, 255, "SECRET_OCR_TEXT", 12, "3 Tr ") + text(20, 360, "KEEP_PUBLIC_OCR_PAGE")
    return simple(c, {6: img}, resources_extra="/XObject << /Im1 6 0 R >>")


def leaks_metadata_attachment_js():
    xmp = b'<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><dc:title>SECRET_XMP_TITLE</dc:title></x:xmpmeta><?xpacket end="w"?>'
    ex = {6: "<< /Title (SECRET_INFO_TITLE) /Author (SECRET_INFO_AUTHOR) /Producer (SECRET_PRODUCER) >>",
          7: stream("/Type /Metadata /Subtype /XML", xmp),
          8: "<< /Type /Filespec /F (SECRET_ATTACHMENT_NAME.txt) /UF (SECRET_ATTACHMENT_NAME.txt) /EF << /F 9 0 R >> >>",
          9: stream("/Type /EmbeddedFile", b"SECRET_ATTACHMENT_BODY", True),
          10: "<< /S /JavaScript /JS (SECRET_NAMED_JS) >>",
          11: "<< /S /JavaScript /JS (app.alert\\('SECRET_OPENACTION_JS'\\)) >>",
          12: "<< /S /JavaScript /JS (SECRET_AA_JS) >>"}
    cat = ("/Metadata 7 0 R /OpenAction 11 0 R /AA << /WC 12 0 R >> /Names << /EmbeddedFiles << /Names [(SECRET_ATTACHMENT_NAME.txt) 8 0 R] >> "
           "/JavaScript << /Names [(n) 10 0 R] >> >>").replace(">> >>", ">> >>", 1)
    return simple(text(20, 360, "KEEP_PUBLIC_KITCHEN"), ex, cat_extra=cat, trailer="/Info 6 0 R")


def incremental_prior_text():
    base, x1 = build({1: "<< /Type /Catalog /Pages 2 0 R >>", 2: "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                      3: "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %d %d] /Contents 4 0 R /Resources << %s >> >>" % (W, H, FONT_RES),
                      4: stream("", text(20, 300, "SECRET_OLD_REVISION")),
                      5: "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"})
    return build({4: stream("", text(20, 300, "KEEP_NEW_REVISION"))}, base=base, prev=x1)[0]


def acroform_values():
    ap = _form(text(4, 10, "SECRET_FORM_VALUE", 12), bbox="0 0 200 30")
    w = ("<< /Type /Annot /Subtype /Widget /FT /Tx /T (SECRET_FORM_NAME) /V (SECRET_FORM_VALUE) /DV (SECRET_FORM_DEFAULT) "
         "/Rect [20 300 220 330] /F 4 /P 3 0 R /DA (/F1 12 Tf 0 g) /AP << /N 7 0 R >> >>")
    cat = "/AcroForm << /Fields [6 0 R] /DA (/F1 12 Tf 0 g) /DR << %s >> >>" % FONT_RES
    return simple(text(20, 360, "KEEP_PUBLIC_FORM"), {6: w, 7: ap}, page_extra="/Annots [6 0 R]", cat_extra=cat)


def annot_appearance():
    ap = _form(text(4, 10, "SECRET_ANNOT_TEXT", 12), bbox="0 0 180 40")
    ann = ("<< /Type /Annot /Subtype /FreeText /Rect [20 280 200 320] /Contents (SECRET_ANNOT_CONTENTS) /NM (SECRET_ANNOT_NAME) "
           "/T (SECRET_ANNOT_AUTHOR) /DA (/F1 12 Tf 0 g) /F 4 /AP << /N 7 0 R >> >>")
    link = "<< /Type /Annot /Subtype /Link /Rect [20 200 100 220] /Border [0 0 0] /A << /S /URI /URI (https://example.invalid/SECRET_LINK_URI) >> >>"
    return simple(text(20, 360, "KEEP_PUBLIC_ANNOT"), {6: ann, 7: ap, 8: link}, page_extra="/Annots [6 0 R 8 0 R]")


def mixed_regions():
    """Text, image, vector and Form secrets on one page, each under its own region; public text and a public image bar stay."""
    img = stream("/Type /XObject /Subtype /Image /Width 64 /Height 32 /ColorSpace /DeviceRGB /BitsPerComponent 8", _bar_image(b"\xb4\x14\x14"), True)
    form = _form(text(0, 0, "SECRET_MIXED_FORM", 12), res="/Font << /F1 5 0 R >>", bbox="0 -5 200 20")
    c = (b"q 200 0 0 100 20 200 cm /Im1 Do Q\n" + b"q 0.9 0.1 0.1 rg 220 60 60 60 re f Q\n" + b"q 1 0 0 1 20 40 cm /Fm1 Do Q\n"
         + text(20, 330, "SECRET_MIXED_TEXT") + text(20, 370, "KEEP_PUBLIC_MIXED"))
    return simple(c, {6: img, 7: form}, resources_extra="/XObject << /Im1 6 0 R /Fm1 7 0 R >>")


def _icc(tail):
    p = bytearray(132)
    p[0:4] = (132).to_bytes(4, "big")
    p[16:20], p[36:40] = b"RGB ", b"acsp"
    return bytes(p) + tail


def icc_tail_secret():
    c = b"/CS1 cs 0.2 0.4 0.6 sc 20 20 200 100 re f\n" + text(20, 300, "SECRET_ICC_TEXT") + text(20, 370, "KEEP_PUBLIC_ICC")
    return simple(c, {6: stream("/N 3", _icc(b"SECRET_ICC_TAIL"))}, resources_extra="/ColorSpace << /CS1 [/ICCBased 6 0 R] >>")


def jpeg_payload_secret():
    import fitz  # dev-only: only to encode one small JPEG
    px = bytes((x * 4 + y * 3 + c * 60) % 256 for y in range(32) for x in range(64) for c in range(3))
    j = fitz.Pixmap(fitz.csRGB, 64, 32, px, False).tobytes("jpeg")
    com, app = b"SECRET_JPEG_COM", b"SECRET_JPEG_APP2"
    j = j[:2] + b"\xff\xfe" + (len(com) + 2).to_bytes(2, "big") + com + b"\xff\xe2" + (len(app) + 2).to_bytes(2, "big") + app + j[2:] + b"SECRET_JPEG_TAIL"
    img = stream("/Type /XObject /Subtype /Image /Width 64 /Height 32 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode", j)
    c = b"q 200 0 0 100 20 200 cm /Im1 Do Q\n" + text(20, 300, "SECRET_JPEG_TEXT") + text(20, 370, "KEEP_PUBLIC_JPEG")
    return simple(c, {6: img}, resources_extra="/XObject << /Im1 6 0 R >>", flate=False)


def font_payload_secret():
    """HDDN's embedded-font fixture with a marker written into the TrueType name table (dev-only: pikepdf rewrites one stream)."""
    import io
    import struct
    import pikepdf
    p = pikepdf.open(OUT.parent / "hddn" / "unicode-glyph-widths.pdf")
    for o in p.objects:
        if o is not None and o._type_code == pikepdf.ObjectType.stream and "/Length1" in o:
            d = bytearray(o.read_bytes())
            for i in range(struct.unpack(">H", d[4:6])[0]):
                tag, _, off, ln = struct.unpack(">4sIII", d[12 + 16 * i:28 + 16 * i])
                if tag == b"name":
                    m = b"SECRET_FONT_NAME_TABLE"
                    d[off + ln - 100:off + ln - 100 + len(m)] = m
            o.write(zlib.compress(bytes(d), 9), filter=pikepdf.Name.FlateDecode)
    out = io.BytesIO()
    p.save(out, static_id=True, compress_streams=False, object_stream_mode=pikepdf.ObjectStreamMode.disable)
    return out.getvalue()


def _encrypted(mode):
    import fitz  # PyMuPDF, dev-only
    plain = simple(text(20, 360, "KEEP_PUBLIC_ENCRYPTED") + text(20, 330, "SECRET_ENCRYPTED_BODY"), flate=False)
    doc = fitz.open("pdf", plain)
    out = doc.tobytes(encryption=getattr(fitz, "PDF_ENCRYPT_" + mode), user_pw=ENC_USER, owner_pw=ENC_OWNER, permissions=-1, garbage=0)
    doc.close()
    return out


def R(x, y, s, size=14, kind="sensitive"):
    return {"kind": kind, "page": 0, "rectTopLeft": region(x, y, s, size), "text": s}


K = {"mustKeepText": [], "mustRemove": []}
# id -> (generator, features, expected, extra meta)
FIXTURES = {
    "corrupt-flate": (corrupt_flate, ["corrupt-flate", "malformed"],
                      {"behavior": "content stream fails to inflate: decode error must be surfaced (error or explicit warning), never a silent pass", "mustRemove": ["SECRET_CORRUPT_FLATE"], "mustKeepText": []}, {}),
    "forms-cyclic": (forms_cyclic, ["form-xobject", "cyclic-form"],
                     {"behavior": "cycle A->B->A must hit the nesting limit/cycle guard, not recurse", "mustRemove": ["SECRET_CYCLIC_A", "SECRET_CYCLIC_B"], "mustKeepText": ["KEEP_PUBLIC_CYCLIC"],
                      "regions": [R(20, 360, "KEEP_PUBLIC_CYCLIC", kind="public")]}, {}),
    "forms-deep-40": (forms_deep, ["form-xobject", "deep-form"],
                      {"behavior": "40 nested Forms exceed maxNesting 32: LIMIT_EXCEEDED, no output", "mustRemove": ["SECRET_DEEP_FORM_40"], "mustKeepText": ["KEEP_PUBLIC_DEEP"],
                       "regions": [R(20, 200, "SECRET_DEEP_FORM_40"), R(20, 360, "KEEP_PUBLIC_DEEP", kind="public")]}, {}),
    "form-reuse-matrices": (form_reuse_matrices, ["form-xobject", "shared-form", "rotation-matrix"],
                            {"behavior": "one Form drawn 3x (identity, 0.5 scale, 90deg) plus own /Matrix; redacting one occurrence must not alter the others", "mustRemove": ["SECRET_FORM_REUSE"], "mustKeepText": ["KEEP_PUBLIC_FORM_REUSE"],
                             "regions": [R(26, 306, "SECRET_FORM_REUSE", 18), R(23, 253, "SECRET_FORM_REUSE", 9), R(20, 360, "KEEP_PUBLIC_FORM_REUSE", kind="public")],
                             "regionsNote": "first two occurrences only; third (rotated) not mapped"}, {}),
    "many-pages-1001": (many_pages, ["many-pages", "shared-content-stream"],
                        {"behavior": "1001 pages: above SDK default maxPages 1000 (LIMIT_EXCEEDED unless raised); one shared content stream, no per-page markers", "mustRemove": [], "mustKeepText": ["PAGE_COMMON_TEXT"]}, {}),
    "ocg-hidden-text": (ocg_hidden, ["ocg", "hidden-ocg-text"],
                        {"behavior": "default config /D has layer 1 OFF; its text must not survive layer flattening", "mustRemove": ["SECRET_HIDDEN_OCG", "SECRET_LAYER_OFF_NAME"],
                         "mustKeepText": ["KEEP_PUBLIC_OCG", "KEEP_PUBLIC_ON_LAYER"],
                         "regions": [R(20, 280, "SECRET_HIDDEN_OCG"), R(20, 320, "KEEP_PUBLIC_ON_LAYER", kind="public")]}, {}),
    "ocr-over-image": (ocr_over_image, ["invisible-text", "image", "ocr-layer"],
                       {"behavior": "image with secret pixels under invisible (Tr 3) OCR text; both must go under the region; gray bar must survive",
                        "mustRemove": ["SECRET_OCR_TEXT"], "mustKeepText": ["KEEP_PUBLIC_OCR_PAGE"],
                        "regions": [{"kind": "sensitive-image", "page": 0, "rectTopLeft": [45, 125, 145, 150], "text": "black bar pixels cols 8-40 rows 8-16 of 64x32"},
                                    {"kind": "public-image", "page": 0, "rectTopLeft": [157.5, 125, 195, 150], "text": "gray 0x80 bar must stay"},
                                    R(45, 255, "SECRET_OCR_TEXT", 12), R(20, 360, "KEEP_PUBLIC_OCR_PAGE", kind="public")]}, {}),
    "leaks-metadata-attachment-js": (leaks_metadata_attachment_js, ["metadata", "xmp", "embedded-file", "javascript", "openaction", "additional-actions"],
                                     {"behavior": "Info+XMP+attachment+named JS+OpenAction+catalog AA; sanitizer must drop all",
                                      "mustRemove": ["SECRET_INFO_TITLE", "SECRET_INFO_AUTHOR", "SECRET_PRODUCER", "SECRET_XMP_TITLE", "SECRET_ATTACHMENT_NAME.txt", "SECRET_ATTACHMENT_BODY",
                                                     "SECRET_NAMED_JS", "SECRET_OPENACTION_JS", "SECRET_AA_JS"], "mustKeepText": ["KEEP_PUBLIC_KITCHEN"]}, {}),
    "incremental-prior-text": (incremental_prior_text, ["incremental-update", "prior-revision"],
                               {"behavior": "rev1 text lives only in the first revision's replaced object; output must contain no /Prev and no old bytes",
                                "mustRemove": ["SECRET_OLD_REVISION"], "mustKeepText": ["KEEP_NEW_REVISION"]}, {}),
    "acroform-values": (acroform_values, ["form", "widget", "appearance-stream"],
                        {"behavior": "Tx field with /V, /DV and /AP; flatten then drop AcroForm and values",
                         "mustRemove": ["SECRET_FORM_NAME", "SECRET_FORM_VALUE", "SECRET_FORM_DEFAULT"], "mustKeepText": ["KEEP_PUBLIC_FORM"],
                         "regions": [{"kind": "sensitive", "page": 0, "rectTopLeft": [20, 70, 220, 100], "text": "widget rect"}, R(20, 360, "KEEP_PUBLIC_FORM", kind="public")]}, {}),
    "annot-appearance": (annot_appearance, ["annotation", "appearance-stream", "link"],
                         {"behavior": "FreeText with /AP /N plus URI link; flatten drops /Contents /NM /T and the link",
                          "mustRemove": ["SECRET_ANNOT_CONTENTS", "SECRET_ANNOT_NAME", "SECRET_ANNOT_AUTHOR", "SECRET_ANNOT_TEXT", "SECRET_LINK_URI"], "mustKeepText": ["KEEP_PUBLIC_ANNOT"],
                         "regions": [{"kind": "sensitive", "page": 0, "rectTopLeft": [20, 80, 200, 120], "text": "annot rect"}, R(20, 360, "KEEP_PUBLIC_ANNOT", kind="public")]}, {}),
    "mixed-regions": (mixed_regions, ["text", "image", "vector", "form-xobject"],
                      {"behavior": "text, image pixels, a vector rectangle and Form text each under a region; public text and the gray image bar stay",
                       "mustRemove": ["SECRET_MIXED_TEXT", "SECRET_MIXED_FORM"], "mustKeepText": ["KEEP_PUBLIC_MIXED"],
                       "regions": [R(20, 330, "SECRET_MIXED_TEXT"), R(20, 40, "SECRET_MIXED_FORM", 12),
                                   {"kind": "sensitive-image", "page": 0, "rectTopLeft": [45, 125, 145, 150], "text": "red-brown bar pixels (180,20,20)"},
                                   {"kind": "public-image", "page": 0, "rectTopLeft": [157.5, 125, 195, 150], "text": "gray bar stays"},
                                   {"kind": "sensitive-vector", "page": 0, "rectTopLeft": [219, 279, 281, 341], "text": "red square"},
                                   R(20, 370, "KEEP_PUBLIC_MIXED", kind="public")]}, {}),
    "icc-tail-secret": (icc_tail_secret, ["icc-profile", "vector"],
                        {"behavior": "ICCBased stream with bytes after the declared profile size", "mustRemove": ["SECRET_ICC_TEXT", "SECRET_ICC_TAIL"], "mustKeepText": ["KEEP_PUBLIC_ICC"],
                         "regions": [R(20, 300, "SECRET_ICC_TEXT"), R(20, 370, "KEEP_PUBLIC_ICC", kind="public")]}, {}),
    "jpeg-payload-secret": (jpeg_payload_secret, ["jpeg", "image", "image-payload"],
                            {"behavior": "DCT image with COM, APP2 and bytes after EOI", "mustRemove": ["SECRET_JPEG_TEXT", "SECRET_JPEG_COM", "SECRET_JPEG_APP2", "SECRET_JPEG_TAIL"],
                             "mustKeepText": ["KEEP_PUBLIC_JPEG"], "regions": [R(20, 300, "SECRET_JPEG_TEXT"), R(20, 370, "KEEP_PUBLIC_JPEG", kind="public"),
                                                                                                  {"kind": "sensitive-image", "page": 0, "rectTopLeft": [45, 125, 145, 150], "text": "JPEG pixels"}]}, {"deterministic": False}),
    "font-payload-secret": (font_payload_secret, ["font-payload", "embedded-font", "non-bmp"],
                            {"behavior": "TrueType program whose name table carries a marker; SECRET_UNICODE is drawn text", "mustRemove": ["SECRET_UNICODE", "SECRET_FONT_NAME_TABLE"],
                             "mustKeepText": ["KEEP_PUBLIC_UNICODE_GLYPH_WIDTHS"]}, {}),
}
for _id, _mode, _alg in (("encrypted-rc4-40", "RC4_40", "RC4-40"), ("encrypted-rc4-128", "RC4_128", "RC4-128"),
                         ("encrypted-aes-128", "AES_128", "AES-128"), ("encrypted-aes-256", "AES_256", "AES-256")):
    FIXTURES[_id] = ((lambda m=_mode: _encrypted(m)), ["encrypted", _alg.lower()],
                     {"behavior": "needs password; user/owner both work; output never re-encrypted", "mustRemove": ["SECRET_ENCRYPTED_BODY"], "mustKeepText": ["KEEP_PUBLIC_ENCRYPTED"]},
                     {"password": ENC_USER, "ownerPassword": ENC_OWNER, "deterministic": False})


def main():
    force = "--force" in sys.argv
    OUT.mkdir(exist_ok=True)
    for i, (fn, _f, _e, meta) in FIXTURES.items():
        p = OUT / (i + ".pdf")
        if meta.get("deterministic") is False and p.exists() and not force:
            continue
        data = fn()
        if not p.exists() or p.read_bytes() != data:
            p.write_bytes(data)
            print("wrote", p.name, len(data))


if __name__ == "__main__":
    main()
