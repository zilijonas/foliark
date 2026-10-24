#!/usr/bin/env python3
"""Foliark synthetic fixtures beyond HDDN's 21 (NEW-6). Output: tests/fixtures/generated/<id>.pdf

Pinned: Python 3.9, PyMuPDF==1.26.5 (dev-only, used ONLY to encrypt; everything else is hand-written PDF syntax
from pdfsyn.py and byte-deterministic). Encrypted files embed random salts/IVs, so they are written only when
absent (or with --force) and manifest.json (sha256) is the source of truth for them.

  python3 generate.py            write missing/changed files
  python3 generate.py --force    also regenerate the nondeterministic encrypted ones
"""
import pathlib
import re
import struct
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


# --- cjk-predefined-cmaps (NEW-88) ---------------------------------------------------------------------------------
# Type0 fonts whose Encoding is a predefined CMap from packages/sdk/cmaps (Adobe, BSD-3). The embedded TrueType program is
# synthetic (one box glyph per character, hand-built, public domain); CIDToGIDMap maps the Adobe CID, read here from the
# pack file with an independent parser, to that glyph. No ToUnicode: the Unicode text comes from the encoding alone.
PACK = pathlib.Path(__file__).resolve().parents[3] / "packages/sdk/cmaps"
GB_TEXT, JP_TEXT, SONG_TEXT = "\u4e2d\u6587\u6d4b\u8bd5", "\u65e5\u672c\u8a9e", "\u4f60\u597d"


def _cid_table(name):
    t = {}
    body = (PACK / (name + ".cmap")).read_text()
    for kind, blk in re.findall(r"begin(cidrange|cidchar)\n(.*?)end\1", body, re.S):
        for line in blk.strip().splitlines():
            f = line.split()
            lo = int(f[0].strip("<>"), 16)
            hi, cid = (int(f[1].strip("<>"), 16), int(f[2])) if kind == "cidrange" else (lo, int(f[1]))
            t.update({c: cid + c - lo for c in range(lo, hi + 1)})
    return t


def _cmap4(mapping):
    """cmap table with one (3,1) format-4 subtable, one segment per {character: glyph id} entry."""
    pairs = sorted((ord(c), g) for c, g in mapping.items()) + [(0xFFFF, 0)]
    n = len(pairs)
    k = n.bit_length() - 1
    body = struct.pack(">HHHH", 2 * n, 2 << k, k, 2 * n - (2 << k))
    body += b"".join(struct.pack(">H", c) for c, _ in pairs) + b"\0\0" + b"".join(struct.pack(">H", c) for c, _ in pairs)
    body += b"".join(struct.pack(">H", (g - c) & 0xFFFF if c != 0xFFFF else 1) for c, g in pairs) + b"\0\0" * n
    sub = struct.pack(">HHH", 4, 6 + len(body), 0) + body
    return struct.pack(">HHHHI", 0, 1, 3, 1, 12) + sub


def _box_ttf(n, cmap=None, blank=(), adv=1000):
    """TrueType with .notdef plus n-1 box glyphs (advance `adv`, 50..950 x 0..800), tables head hhea maxp hmtx loca glyf;
    with `cmap` ({character: glyph id}) also a Unicode cmap; glyph ids in `blank` have no outline (a space)."""
    box = struct.pack(">hhhhh", 1, 50, 0, 950, 800) + struct.pack(">H", 3) + struct.pack(">H", 0) + bytes([1] * 4)
    box += struct.pack(">4h", 50, 0, 900, 0) + struct.pack(">4h", 0, 800, 0, -800)
    sizes = [0 if i in blank else len(box) for i in range(n)]
    glyf = box * (n - len(blank))
    loca = b"".join(struct.pack(">H", sum(sizes[:i]) // 2) for i in range(n + 1))
    head = struct.pack(">IIIIHHqqhhhhHHhhh", 0x10000, 0x10000, 0, 0x5F0F3CF5, 0, 1000, 0, 0, 50, 0, 950, 800, 0, 8, 2, 0, 0)
    hhea = struct.pack(">IhhhHhhhhhhhhhhhH", 0x10000, 800, -200, 0, 1000, 50, 50, 950, 1, 0, 0, 0, 0, 0, 0, 0, n)
    maxp = struct.pack(">IH", 0x10000, n) + struct.pack(">13H", 4, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0)
    hmtx = struct.pack(">hh", adv, 50) * n
    tables = {"head": head, "hhea": hhea, "maxp": maxp, "hmtx": hmtx, "loca": loca, "glyf": glyf}
    if cmap:
        tables["cmap"] = _cmap4(cmap)
    tables = sorted(tables.items())
    out, offset = b"", 12 + 16 * len(tables)
    directory = struct.pack(">IHHHH", 0x10000, len(tables), 64, 2, 0)
    for tag, data in tables:
        pad = data + b"\0" * (-len(data) % 4)
        csum = sum(struct.unpack(">%dI" % (len(pad) // 4), pad)) & 0xFFFFFFFF
        directory += tag.encode() + struct.pack(">III", csum, offset + len(out), len(data))
        out += pad
    return directory + out


def cjk_predefined_cmaps():
    chars = GB_TEXT + JP_TEXT  # glyph k+1 draws chars[k]
    ttf = _box_ttf(len(chars) + 1)

    def cid_to_gid(name, text):
        tab = _cid_table(name)
        gids = {tab[ord(c)]: chars.index(c) + 1 for c in text}
        return stream("", b"".join(struct.pack(">H", gids.get(i, 0)) for i in range(max(gids) + 1)), flate=True)

    def hexs(text):
        return "<" + "".join("%04X" % ord(c) for c in text) + ">"

    def cid_font(base, ordering, supplement, desc, extra=""):
        return ("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /%s /CIDSystemInfo << /Registry (Adobe) /Ordering (%s) /Supplement %d >> "
                "/FontDescriptor %d 0 R /DW 1000 %s >>" % (base, ordering, supplement, desc, extra))

    def descriptor(base, flags, file=""):
        return ("<< /Type /FontDescriptor /FontName /%s /Flags %d /FontBBox [50 0 950 800] /ItalicAngle 0 /Ascent 800 /Descent -200 "
                "/CapHeight 800 /StemV 80 %s >>" % (base, flags, file))

    ex = {
        6: "<< /Type /Font /Subtype /Type0 /BaseFont /FoliarkBox-GB /Encoding /UniGB-UCS2-H /DescendantFonts [9 0 R] >>",
        7: "<< /Type /Font /Subtype /Type0 /BaseFont /FoliarkBox-JP /Encoding /UniJIS-UCS2-H /DescendantFonts [10 0 R] >>",
        8: "<< /Type /Font /Subtype /Type0 /BaseFont /STSong-Light /Encoding /UniGB-UCS2-H /DescendantFonts [11 0 R] >>",
        9: cid_font("FoliarkBox-GB", "GB1", 6, 12, "/CIDToGIDMap 14 0 R"),
        10: cid_font("FoliarkBox-JP", "Japan1", 7, 13, "/CIDToGIDMap 15 0 R"),
        11: cid_font("STSong-Light", "GB1", 6, 16).replace("CIDFontType2", "CIDFontType0"),
        12: descriptor("FoliarkBox-GB", 4, "/FontFile2 17 0 R"),
        13: descriptor("FoliarkBox-JP", 4, "/FontFile2 18 0 R"),
        14: cid_to_gid("UniGB-UCS2-H", GB_TEXT),
        15: cid_to_gid("UniJIS-UCS2-H", JP_TEXT),
        16: descriptor("STSong-Light", 6),
        17: stream("/Length1 %d" % len(ttf), ttf, flate=True),
        18: stream("/Length1 %d" % len(ttf), ttf, flate=True),  # a program of its own: redaction rewrites per program
    }
    content = (text(20, 360, "KEEP_PUBLIC_CJK") + ("BT /FG 24 Tf 20 300 Td %s Tj ET\nBT /FJ 24 Tf 20 240 Td %s Tj ET\n"
               "BT /FS 24 Tf 20 180 Td %s Tj ET\n" % (hexs(GB_TEXT), hexs(JP_TEXT), hexs(SONG_TEXT))).encode())
    return simple(content, ex, fonts="/F1 5 0 R /FG 6 0 R /FJ 7 0 R /FS 8 0 R")


# --- real-document font shapes (NEW-100) ---------------------------------------------------------------------------
# iOS Quartz: Type0 /Identity-H over an embedded TrueType whose own Unicode cmap is the only source of text: the
# ToUnicode CMap declares a codespace and no mapping, there is no CIDToGIDMap (identity) and spaces are drawn glyphs.
QUARTZ_SECRET, QUARTZ_DATE, QUARTZ_KEEP = "SECRET_QUARTZ", "2026-10-23", "KEEP_QUARTZ_LINE"


def _cid_hex(text, chars):
    return "<" + "".join("%04X" % (chars.index(c) + 1) for c in text) + ">"


def quartz_type0_empty_tounicode():
    chars = "".join(sorted(set(QUARTZ_SECRET + QUARTZ_DATE + QUARTZ_KEEP + " ")))
    cmap = {c: k + 1 for k, c in enumerate(chars)}
    # Like Times New Roman: NBSP and soft hyphen share the space and hyphen glyphs; the lowest code point names the glyph.
    cmap.update({"\u00a0": cmap[" "], "\u00ad": cmap["-"]})
    ttf = _box_ttf(len(chars) + 1, cmap=cmap, blank={cmap[" "]})
    tu = (b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) "
          b"/Supplement 0 >> def /CMapName /Adobe-Identity-UCS def /CMapType 2 def\n1 begincodespacerange\n<0000><FFFF>\n"
          b"endcodespacerange\nendcmap CMapName currentdict /CMap defineresource pop end end\n")
    ex = {
        6: "<< /Type /Font /Subtype /Type0 /BaseFont /AAAAAB+FoliarkBox /Encoding /Identity-H /DescendantFonts [7 0 R] /ToUnicode 9 0 R >>",
        7: ("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /AAAAAB+FoliarkBox /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) "
            "/Supplement 0 >> /W [1 [%s]] /DW 1000 /FontDescriptor 8 0 R >>" % " ".join(["1000"] * len(chars))),
        8: ("<< /Type /FontDescriptor /FontName /AAAAAB+FoliarkBox /Flags 4 /FontBBox [50 0 950 800] /ItalicAngle 0 /Ascent 800 "
            "/Descent -200 /CapHeight 800 /StemV 80 /FontFile2 10 0 R >>"),
        9: stream("", tu, flate=True),
        10: stream("/Length1 %d" % len(ttf), ttf, flate=True),
    }
    content = (text(20, 360, "KEEP_PUBLIC_QUARTZ") +
               ("BT /FQ 10 Tf 20 300 Td %s Tj ET\nBT /FQ 10 Tf 20 270 Td %s Tj ET\n"
                % (_cid_hex(QUARTZ_SECRET + " " + QUARTZ_DATE, chars), _cid_hex(QUARTZ_KEEP, chars))).encode())
    return simple(content, ex, fonts="/F1 5 0 R /FQ 6 0 R")


# iOS Quartz again, with decomposed accents: a combining mark is its own glyph of advance 0 (/W 0), drawn over its base through a
# TJ kern back and forth, as Quartz writes "ą" (NEW-102). It has no width, so no box of its own: it belongs to its base.
MARKS_SECRET, MARKS_KEEP = "SECRET_A\u0328_E\u0301X", "KEEP_E\u0301_MARK"
OGONEK, ACUTE = "\u0328", "\u0301"


def quartz_type0_combining_marks():
    chars = "".join(sorted(set(MARKS_SECRET + MARKS_KEEP)))
    cmap = {c: k + 1 for k, c in enumerate(chars)}
    ttf = _box_ttf(len(chars) + 1, cmap=cmap)
    tu = (b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) "
          b"/Supplement 0 >> def /CMapName /Adobe-Identity-UCS def /CMapType 2 def\n1 begincodespacerange\n<0000><FFFF>\n"
          b"endcodespacerange\nendcmap CMapName currentdict /CMap defineresource pop end end\n")
    widths = " ".join("0" if c in (OGONEK, ACUTE) else "1000" for c in chars)

    def tj(word):  # a mark follows its base: back one em, the mark (advance 0), forward one em
        return "[" + "".join(("1000 <%04X> -1000 " if c in (OGONEK, ACUTE) else "<%04X> ") % (chars.index(c) + 1) for c in word) + "] TJ"

    ex = {
        6: "<< /Type /Font /Subtype /Type0 /BaseFont /AAAAAE+FoliarkBox /Encoding /Identity-H /DescendantFonts [7 0 R] /ToUnicode 9 0 R >>",
        7: ("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /AAAAAE+FoliarkBox /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) "
            "/Supplement 0 >> /W [1 [%s]] /DW 1000 /FontDescriptor 8 0 R >>" % widths),
        8: ("<< /Type /FontDescriptor /FontName /AAAAAE+FoliarkBox /Flags 4 /FontBBox [50 0 950 800] /ItalicAngle 0 /Ascent 800 "
            "/Descent -200 /CapHeight 800 /StemV 80 /FontFile2 10 0 R >>"),
        9: stream("", tu, flate=True),
        10: stream("/Length1 %d" % len(ttf), ttf, flate=True),
    }
    content = (text(20, 360, "KEEP_PUBLIC_MARKS") +
               ("BT /FQ 10 Tf 20 300 Td %s ET\nBT /FQ 10 Tf 20 270 Td %s ET\n" % (tj(MARKS_SECRET), tj(MARKS_KEEP))).encode())
    return simple(content, ex, fonts="/F1 5 0 R /FQ 6 0 R")


# iOS Quartz once more (NEW-104): /W rounds each advance up to 1000 while the program's is 999, and the content stream draws
# the program's advance through a TJ number of 1 between glyphs, so every line is narrower on the page than its /W widths
# add up to. A paragraph of three lines that each fill the box (the next word never fits), the KEEP line close below it,
# a word beside both boxes (a second column), a one-word box, and an ogonek drawn as Quartz draws it: a zero-advance mark right after its base,
# no kern. The subset has no precomposed letter.
TIGHT_ROWS = ["ALPHA BRAVO", "DELTA ECHOS", "GOLFS HOTEL"]
TIGHT_WORD, TIGHT_BESIDE, TIGHT_OGONEK, TIGHT_KEEP = "KILO", "LIMA", "ZA\u0328SIS", "KEEP_TIGHT_LINE"


def quartz_type0_tight_widths():
    chars = "".join(sorted(set("".join(TIGHT_ROWS) + TIGHT_WORD + TIGHT_BESIDE + TIGHT_OGONEK + TIGHT_KEEP + " ")))
    cmap = {c: k + 1 for k, c in enumerate(chars)}
    ttf = _box_ttf(len(chars) + 1, cmap=cmap, blank={cmap[" "]}, adv=999)
    tu = (b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) "
          b"/Supplement 0 >> def /CMapName /Adobe-Identity-UCS def /CMapType 2 def\n1 begincodespacerange\n<0000><FFFF>\n"
          b"endcodespacerange\nendcmap CMapName currentdict /CMap defineresource pop end end\n")
    widths = " ".join("0" if c == OGONEK else "1000" for c in chars)

    def tj(x, y, word):  # 1/1000 em back before every glyph that is not a mark: the drawn advance is the program's 999
        body = "".join(("" if i == 0 or c == OGONEK else "1 ") + "<%04X> " % (chars.index(c) + 1) for i, c in enumerate(word))
        return "BT /FQ 10 Tf %d %d Td [%s] TJ ET\n" % (x, y, body)

    ex = {
        6: "<< /Type /Font /Subtype /Type0 /BaseFont /AAAAAF+FoliarkBox /Encoding /Identity-H /DescendantFonts [7 0 R] /ToUnicode 9 0 R >>",
        7: ("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /AAAAAF+FoliarkBox /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) "
            "/Supplement 0 >> /W [1 [%s]] /DW 1000 /FontDescriptor 8 0 R >>" % widths),
        8: ("<< /Type /FontDescriptor /FontName /AAAAAF+FoliarkBox /Flags 4 /FontBBox [50 0 950 800] /ItalicAngle 0 /Ascent 800 "
            "/Descent -200 /CapHeight 800 /StemV 80 /FontFile2 10 0 R >>"),
        9: stream("", tu, flate=True),
        10: stream("/Length1 %d" % len(ttf), ttf, flate=True),
    }
    rows = "".join(tj(20, 300 - 14 * i, r) for i, r in enumerate(TIGHT_ROWS))
    beside = tj(200, 300, TIGHT_BESIDE) + tj(200, 200, TIGHT_BESIDE)  # each box ends where its own text ends
    content = (text(20, 360, "KEEP_PUBLIC_TIGHT") + (rows + beside + tj(20, 252, TIGHT_KEEP) + tj(20, 200, TIGHT_WORD)
                                                     + tj(20, 170, TIGHT_OGONEK)).encode())
    return simple(content, ex, fonts="/F1 5 0 R /FQ 6 0 R")


# Word: each face twice, a TrueType WinAnsi subset for what WinAnsi can encode and a Type0 /Identity-H twin (ToUnicode with
# bfchar and an array-form bfrange) for the rest; a word switches fonts mid-word, one positioned run per font switch.
TWIN_DATE = [("T", "2002 m. bir"), ("U", "\u017e"), ("T", "elio 20 d.")]
TWIN_SECRET = [("T", "SECRET_gegu"), ("U", "\u017e\u0117"), ("T", "s")]
TWIN_U = "\u017e\u0117\u0161\u0104"  # glyph k+1 of the twin's program; ToUnicode maps code k+1 back


def word_winansi_type0_twins():
    ascii_chars = "".join(chr(c) for c in range(32, 127))
    tt = _box_ttf(len(ascii_chars) + 1, cmap={c: k + 1 for k, c in enumerate(ascii_chars)}, blank={1})  # glyph 1 is the space
    cid = _box_ttf(len(TWIN_U) + 1)
    tu = ("/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Adobe-Identity-UCS def /CMapType 2 def\n"
          "1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n2 beginbfchar\n<0001> <017E>\n<0004> <0104>\nendbfchar\n"
          "1 beginbfrange\n<0002> <0003> [<0117> <0161>]\nendbfrange\nendcmap CMapName currentdict /CMap defineresource pop end end\n")
    ex = {
        6: ("<< /Type /Font /Subtype /TrueType /BaseFont /AAAAAC+FoliarkBox /Encoding /WinAnsiEncoding /FirstChar 32 /LastChar 126 "
            "/Widths [%s] /FontDescriptor 8 0 R >>" % " ".join(["1000"] * 95)),
        7: "<< /Type /Font /Subtype /Type0 /BaseFont /AAAAAD+FoliarkBox /Encoding /Identity-H /DescendantFonts [9 0 R] /ToUnicode 11 0 R >>",
        8: ("<< /Type /FontDescriptor /FontName /AAAAAC+FoliarkBox /Flags 32 /FontBBox [50 0 950 800] /ItalicAngle 0 /Ascent 800 "
            "/Descent -200 /CapHeight 800 /StemV 80 /FontFile2 12 0 R >>"),
        9: ("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /AAAAAD+FoliarkBox /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) "
            "/Supplement 0 >> /W [1 [1000 1000 1000 1000]] /CIDToGIDMap /Identity /FontDescriptor 10 0 R >>"),
        10: ("<< /Type /FontDescriptor /FontName /AAAAAD+FoliarkBox /Flags 32 /FontBBox [50 0 950 800] /ItalicAngle 0 /Ascent 800 "
             "/Descent -200 /CapHeight 800 /StemV 80 /FontFile2 13 0 R >>"),
        11: stream("", tu.encode(), flate=True),
        12: stream("/Length1 %d" % len(tt), tt, flate=True),
        13: stream("/Length1 %d" % len(cid), cid, flate=True),
    }

    def line(runs, y):
        out, x = "", 20
        for font, s in runs:
            shown = "(%s)" % s if font == "T" else _cid_hex(s, TWIN_U)
            out += "BT /F%s 10 Tf 1 0 0 1 %d %d Tm [%s] TJ ET\n" % (font, x, y, shown)
            x += 10 * len(s)
        return out.encode()

    content = text(20, 360, "KEEP_PUBLIC_TWINS") + line(TWIN_DATE, 300) + line(TWIN_SECRET, 270)
    return simple(content, ex, fonts="/F1 5 0 R /FT 6 0 R /FU 7 0 R")


def _joined(runs):
    return "".join(s for _, s in runs)


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
    "cjk-predefined-cmaps": (cjk_predefined_cmaps, ["cjk", "predefined-cmap", "embedded-font"],
                             {"behavior": "Type0 fonts with predefined Unicode CMaps (UniGB-UCS2-H, UniJIS-UCS2-H) over a synthetic embedded box font, plus a non-embedded STSong-Light; no ToUnicode. Text needs the CMap pack; extraction equals the Unicode of the codes",
                              "mustRemove": [GB_TEXT[1:3]], "mustKeepText": ["KEEP_PUBLIC_CJK", GB_TEXT[0], GB_TEXT[3], JP_TEXT, SONG_TEXT],
                              "requiredAssets": ["cmap:Adobe-GB1-UCS2", "cmap:UniGB-UCS2-H", "cmap:UniJIS-UCS2-H"],
                              "regions": [{"kind": "sensitive", "page": 0, "rectTopLeft": [44, 78, 92, 104], "text": GB_TEXT[1:3]}, R(20, 360, "KEEP_PUBLIC_CJK", kind="public")]}, {}),
    "quartz-type0-empty-tounicode": (quartz_type0_empty_tounicode, ["embedded-font", "identity-h", "empty-tounicode"],
                                     {"behavior": "iOS Quartz shape: Type0 Identity-H, ToUnicode with a codespace and no mapping, no CIDToGIDMap, drawn space glyphs; the text comes from the embedded TrueType's own Unicode cmap (glyph id back to its code point)",
                                      "mustRemove": [QUARTZ_SECRET], "mustKeepText": ["KEEP_PUBLIC_QUARTZ", QUARTZ_DATE, QUARTZ_KEEP],
                                      "regions": [{"kind": "sensitive", "page": 0, "rectTopLeft": [19, 90, 151, 104], "text": QUARTZ_SECRET},
                                                  R(20, 360, "KEEP_PUBLIC_QUARTZ", kind="public")]}, {}),
    "quartz-type0-combining-marks": (quartz_type0_combining_marks, ["embedded-font", "identity-h", "empty-tounicode", "combining-marks"],
                                     {"behavior": "iOS Quartz shape with decomposed accents: ogonek and acute are zero-advance glyphs drawn over their base; a mark belongs to its base glyph, so redacting the base removes the mark and a page with marks still redacts natively",
                                      "mustRemove": [MARKS_SECRET], "mustKeepText": ["KEEP_PUBLIC_MARKS", MARKS_KEEP],
                                      "regions": [{"kind": "sensitive", "page": 0, "rectTopLeft": [19, 90, 135, 104], "text": MARKS_SECRET},
                                                  R(20, 360, "KEEP_PUBLIC_MARKS", kind="public")]}, {}),
    "quartz-type0-tight-widths": (quartz_type0_tight_widths, ["embedded-font", "identity-h", "empty-tounicode", "combining-marks", "tight-widths"],
                                  {"behavior": "iOS Quartz shape whose /W widths are 1/1000 em wider than the advances the content stream draws (TJ corrections), with an ogonek as a zero-advance mark and no precomposed glyph; unchanged paragraphs fit their edit box with their own line breaks, and the word redacts natively",
                                   "mustRemove": [TIGHT_WORD], "mustKeepText": ["KEEP_PUBLIC_TIGHT", TIGHT_KEEP, TIGHT_BESIDE],
                                   "regions": [{"kind": "sensitive", "page": 0, "rectTopLeft": [19, 191, 61, 203], "text": TIGHT_WORD},
                                               R(20, 360, "KEEP_PUBLIC_TIGHT", kind="public")]}, {}),
    "word-winansi-type0-twins": (word_winansi_type0_twins, ["embedded-font", "identity-h", "font-twins"],
                                 {"behavior": "Word shape: a TrueType WinAnsi subset plus a Type0 Identity-H twin (bfchar + array bfrange ToUnicode) for the non-WinAnsi letters, switching mid-word; words and dates read across the switch",
                                  "mustRemove": [_joined(TWIN_SECRET)], "mustKeepText": ["KEEP_PUBLIC_TWINS", _joined(TWIN_DATE).split()[2]],
                                  "regions": [{"kind": "sensitive", "page": 0, "rectTopLeft": [19, 120, 171, 134], "text": _joined(TWIN_SECRET)},
                                              R(20, 360, "KEEP_PUBLIC_TWINS", kind="public")]}, {}),
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
