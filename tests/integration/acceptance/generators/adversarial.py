#!/usr/bin/env python3
"""Adversarial acceptance fixtures (NEW-59). Hand-written PDF syntax (stdlib + tests/fixtures/generators/pdfsyn.py), deterministic.
`build_all(outdir)` writes <id>.pdf and returns the case list. A case:
  id, secrets (text that must not survive a redaction over it), keep (public words that must stay), regions ([{kind, page, rect
  [x0,y0,x1,y1] display space top-left, text}]), expect {scenario: 'ok' | CODE} (default ok), survivors (secrets that legitimately stay
  at an untouched occurrence), sanitize_keep (secrets the spec lets a sanitize-only export keep), hidden (not in extracted text), note.
Scenarios: region, text, sanitize, raster, raster_sanitize, edit (see acceptance.py).
"""
import json
import pathlib
import struct
import sys
import zlib

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[3] / "fixtures" / "generators"))
from pdfsyn import HDR, W, H, build, simple, stream, text  # noqa: E402

F1 = "/Font << /F1 5 0 R >>"
CASES = []


def rg(x, y, s, size=14, pad=2):
    """Generous top-left display rect over Helvetica text drawn at PDF (x, y)."""
    return [x - pad, round(H - y - 0.9 * size, 1), round(x + 0.72 * size * len(s) + pad, 1), round(H - y + 0.3 * size, 1)]


def reg(rect, kind="sensitive", page=0, t="", color=None):
    return {"kind": kind, "page": page, "rect": rect, "text": t, **({"color": color} if color else {})}


def case(cid, pdf, secrets=(), regions=(), keep=None, **kw):
    CASES.append({"id": cid, "pdf": pdf, "secrets": list(secrets), "regions": list(regions), "keep": keep if keep is not None else ["KEEP_PUBLIC_" + cid.upper().replace("-", "_")],
                  "expect": kw.pop("expect", {}), "survivors": kw.pop("survivors", []), "sanitize_keep": kw.pop("sanitize_keep", []),
                  "hidden": kw.pop("hidden", False), "retained": kw.pop("retained", []), "note": kw.pop("note", ""), "password": None, "flags": kw.pop("flags", {})})
    assert not kw, kw


def pub(cid):
    return text(20, 370, "KEEP_PUBLIC_" + cid.upper().replace("-", "_"))


def form(content, bbox="0 0 300 400", res=F1, matrix=""):
    return stream("/Type /XObject /Subtype /Form /BBox [%s] %s /Resources << %s >>" % (bbox, matrix, res), content)


def one(cid, body, secrets, extra=None, **kw):
    """Single-secret-line page: `body` is the secret content drawn at y=300; region covers the first secret."""
    pdf = simple(pub(cid) + body, extra, **{k: kw.pop(k) for k in ("page_extra", "cat_extra", "resources_extra", "trailer", "fonts") if k in kw})
    case(cid, pdf, secrets, kw.pop("regions", [reg(rg(20, 300, secrets[0] if secrets else "x" * 12))]), **kw)


def tj(parts, y=300, size=14, pre=""):
    return ("BT /F1 %d Tf %s20 %d Td %s TJ ET\n" % (size, pre, y, parts)).encode()


def lit(s):
    return "(" + s.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)") + ")"


def img_obj(w, h, data, extra="", cs="/DeviceGray"):
    return stream("/Type /XObject /Subtype /Image /Width %d /Height %d /ColorSpace %s /BitsPerComponent 8 %s" % (w, h, cs, extra), data, True)


def build_all(out):
    out = pathlib.Path(out)
    out.mkdir(parents=True, exist_ok=True)
    CASES.clear()
    # ---- text placement
    one("tj-kerning", tj("[(SEC) 30 (RET_) -40 (TJ) 12 (KERN)]"), ["SECRET_TJKERN"])
    one("split-ops", b"BT /F1 14 Tf 20 300 Td (SECRET_SPLIT) Tj (_TWOOPS) Tj ET\n", ["SECRET_SPLIT_TWOOPS"])
    one("hex-escaped", b"BT /F1 14 Tf 20 300 Td <" + b"SECRET_HEXSTR".hex().encode() + b"> Tj 0 -30 Td (\\123ECRET_OCT\\137ESC) Tj ET\n", ["SECRET_HEXSTR", "SECRET_OCT_ESC"],
        regions=[reg(rg(20, 300, "SECRET_HEXSTR")), reg(rg(20, 270, "SECRET_OCT_ESC"))])
    one("quote-ops", b"BT /F1 14 Tf 16 TL 20 300 Td (first line) ' (SECRET_QUOTE_ONE) ' 3 1 (SECRET_DQUOTE_TWO) \" ET\n", ["SECRET_QUOTE_ONE", "SECRET_DQUOTE_TWO"],
        regions=[reg(rg(20, 284, "SECRET_QUOTE_ONE")), reg(rg(20, 268, "SECRET_DQUOTE_TWO"))], flags={"pypdf_blind": True})
    one("render-modes", b"".join(("BT /F1 14 Tf %d Tr 20 %d Td (%s) Tj ET\n" % (m, y, s)).encode() for m, y, s in
                                  [(1, 300, "SECRET_TR_STROKE"), (2, 270, "SECRET_TR_BOTH"), (3, 240, "SECRET_TR_INVIS"), (7, 210, "SECRET_TR_CLIP")]),
        ["SECRET_TR_STROKE", "SECRET_TR_BOTH", "SECRET_TR_INVIS", "SECRET_TR_CLIP"],
        regions=[reg(rg(20, y, "SECRET_TR_STROKE")) for y in (300, 270, 240, 210)])
    one("spacing-neighbors", b"BT /F1 12 Tf 2 Tc 5 Tw 50 Tz 3 Ts 20 300 Td [(KEEP_BEFORE ) 80 (SECRET_SPACED) 80 ( KEEP_AFTER)] TJ ET\n", ["SECRET_SPACED"],
        keep=["KEEP_PUBLIC_SPACING_NEIGHBORS", "KEEP_BEFORE", "KEEP_AFTER"], regions=[], note="region from the engine's own secret hit")
    sec = "SECRET_OVERPRINT"
    one("overprint", b"".join(("BT /F1 14 Tf %s %s Td (%s) Tj ET\n" % (x, y, sec)).encode() for x, y in [(20, 300), (20.3, 300), (20, 299.5), (19.5, 300)]) + text(20, 300, sec, 14, "3 Tr "), [sec],
        regions=[reg(rg(20, 300, sec))])
    one("white-and-covered", b"BT /F1 14 Tf 1 g 20 300 Td (SECRET_WHITE_INK) Tj ET\n0 g 0 0 0 rg 18 262 220 24 re f 1 g BT /F1 14 Tf 20 268 Td (SECRET_COVERED) Tj ET 0 g\n",
        ["SECRET_WHITE_INK", "SECRET_COVERED"], regions=[reg(rg(20, 300, "SECRET_WHITE_INK")), reg(rg(20, 268, "SECRET_COVERED"))])
    one("clipped-text", b"q 0 0 10 10 re W n BT /F1 14 Tf 20 300 Td (SECRET_CLIPPED_AWAY) Tj ET Q\n", ["SECRET_CLIPPED_AWAY"], note="clip hides it; the region over its box must still remove it", flags={"offpage": True})
    one("invisible-tr3", text(20, 300, "SECRET_INVISIBLE_OCR", 14, "3 Tr "), ["SECRET_INVISIBLE_OCR"], hidden=False)
    one("split-streams", b"", ["SECRET_ACROSS_STREAMS"], regions=[reg(rg(20, 300, "SECRET_ACROSS_STREAMS"))])
    CASES[-1]["pdf"] = build({1: "<< /Type /Catalog /Pages 2 0 R >>", 2: "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                              3: "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents [4 0 R 6 0 R] /Resources << %s >> >>" % F1,
                              4: stream("", pub("split-streams") + b"BT /F1 14 Tf 20 300 Td (SECRET_ACROSS_) Tj"), 5: "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
                              6: stream("", b"(STREAMS) Tj ET")})[0]
    # ligature glyph (one code, two Unicode chars)
    cmap = (b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /T def /CMapType 2 def 1 begincodespacerange <00> <FF> endcodespacerange "
            b"3 beginbfrange <20> <7E> <0020> <01> <01> <00660069> <02> <02> <0066006C> endbfrange endcmap end end")
    cmap = cmap.replace(b"3 beginbfrange <20> <7E> <0020> <01> <01> <00660069> <02> <02> <0066006C> endbfrange", b"1 beginbfrange <20> <7E> <0020> endbfrange 2 beginbfchar <01> <00660069> <02> <0066006C> endbfchar")
    lig = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [1 /fi /fl] >> /ToUnicode 7 0 R >>"
    one("ligature", b"BT /F2 14 Tf 20 300 Td (SECRET_\\001nd \\002ow) Tj ET\n", ["SECRET_find"], extra={6: lig, 7: stream("", cmap)},
        fonts="/F1 5 0 R /F2 6 0 R", regions=[reg(rg(20, 300, "SECRET_fi" + "nd"))], note="fi ligature code maps to two characters")
    # ---- forms and annotations
    fm = form(text(4, 4, "SECRET_FORM_BOTH", 12), "0 0 200 30")
    pdf = simple(pub("form-both-places") + b"q 1 0 0 1 20 300 cm /Fm Do Q\nq 1 0 0 1 20 200 cm /Fm Do Q\n", {6: fm}, resources_extra="/XObject << /Fm 6 0 R >>")
    case("form-both-places", pdf, ["SECRET_FORM_BOTH"], [reg([18, 68, 222, 102])], survivors=["SECRET_FORM_BOTH"], flags={"occ": [[[0], "forbid"], [[1], "require"]]},
         note="one Form drawn inside and outside the region: the outside occurrence must keep its secret, the inside one must lose it")
    for st, cid in (("Off", "annot-as-off"), ("On", "annot-as-on")):
        ex = {6: "<< /Type /Annot /Subtype /Stamp /Rect [20 250 220 280] /F 4 /AP << /N << /On 7 0 R /Off 8 0 R >> >> /AS /%s >>" % st,
              7: form(text(2, 8, "SECRET_AS_ON", 12), "0 0 200 30"), 8: form(text(2, 8, "SECRET_AS_OFF", 12), "0 0 200 30")}
        pdf = simple(pub(cid), ex, page_extra="/Annots [6 0 R]")
        case(cid, pdf, ["SECRET_AS_ON", "SECRET_AS_OFF"], [reg([18, 118, 222, 152], "sensitive")] if st == "On" else [], sanitize_keep=["SECRET_AS_ON"] if st == "On" else [],
             note="/AS " + st + ": the other appearance state must not survive; the drawn one goes under the region")
    ex = {6: form(text(2, 8, "SECRET_ANNOT_POPUP", 12), "0 0 100 30"),
          7: "<< /Type /Annot /Subtype /Text /Rect [20 150 40 170] /Contents (SECRET_ANNOT_NOTE) /T (SECRET_ANNOT_AUTHOR) /Subj (SECRET_ANNOT_SUBJ) /RC (<p>SECRET_ANNOT_RC</p>) /Popup 8 0 R /AP << /N 6 0 R >> /F 4 >>",
          8: "<< /Type /Annot /Subtype /Popup /Rect [60 100 200 160] /Parent 7 0 R /Contents (SECRET_POPUP_CONTENTS) /Open true >>",
          9: "<< /Type /Annot /Subtype /Link /Rect [20 50 100 70] /A << /S /URI /URI (https://example.invalid/SECRET_LINK_URI) >> >>"}
    pdf = simple(pub("annot-popup"), ex, page_extra="/Annots [7 0 R 8 0 R 9 0 R]")
    case("annot-popup", pdf, ["SECRET_ANNOT_POPUP", "SECRET_ANNOT_NOTE", "SECRET_ANNOT_AUTHOR", "SECRET_ANNOT_SUBJ", "SECRET_ANNOT_RC", "SECRET_POPUP_CONTENTS", "SECRET_LINK_URI"],
         [reg([18, 228, 60, 252])], sanitize_keep=["SECRET_ANNOT_POPUP"], flags={"declared_changed": True})
    sm = {6: "<< /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 7 0 R >> >>",
          7: "<< /Type /XObject /Subtype /Form /BBox [0 0 300 400] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /Font << /F1 5 0 R >> >> /Length 0 >>"}
    gtxt = b"1 g BT /F1 14 Tf 20 300 Td (SECRET_SMASK_GROUP) Tj ET\n"
    sm[7] = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Group << /S /Transparency /CS /DeviceGray >> /Resources << %s >>" % F1, gtxt)
    one("smask-group-text", b"q /GS1 gs 0 0 1 rg 0 200 300 200 re f Q\n", ["SECRET_SMASK_GROUP"], sm, resources_extra="/ExtGState << /GS1 6 0 R >>", sanitize_keep=["SECRET_SMASK_GROUP"], hidden=True, expect={"region": "INVALID_ARGUMENT"},
        note="text lives only in a soft-mask group (spec 5.3)")
    # ---- optional content
    ocg = "/OCProperties << /OCGs [6 0 R 7 0 R] /D << /Order [6 0 R 7 0 R] /OFF [7 0 R] >> >>"
    ocex = {6: "<< /Type /OCG /Name (On) >>", 7: "<< /Type /OCG /Name (Off) >>", 8: "<< /Type /OCMD /OCGs [6 0 R 7 0 R] /VE [/And 6 0 R [/Not 7 0 R]] >>"}
    one("ocg-ve", b"/OC /M8 BDC\n" + text(20, 300, "SECRET_OCG_VE") + b"EMC\n", ["SECRET_OCG_VE"], ocex, cat_extra=ocg, resources_extra="/Properties << /M8 8 0 R >>",
        note="spec 8.2 says /VE is refused; the engine evaluates it (And/Not), here to visible: kept by sanitize, removed under a region")
    ocex2 = {6: "<< /Type /OCG /Name (On) >>", 7: "<< /Type /OCG /Name (Off) >>", 8: "<< /Type /OCMD /OCGs [6 0 R] /VE [/Not 6 0 R] >>"}
    one("ocg-ve-hidden", b"/OC /M8 BDC\n" + text(20, 300, "SECRET_VE_HIDDEN") + b"EMC\n", ["SECRET_VE_HIDDEN"], ocex2, cat_extra="/OCProperties << /OCGs [6 0 R 7 0 R] /D << /Order [6 0 R 7 0 R] /OFF [7 0 R] >> >>",
        resources_extra="/Properties << /M8 8 0 R >>", hidden=True, regions=[], flags={"fid_blind": True, "must_gone": True}, note="/VE evaluates to hidden (Not of an ON group); stripping the marker would show it")
    fmo = form(b"/OC /M7 BDC\n" + text(20, 300, "SECRET_OCG_IN_FORM") + b"EMC\n", "0 0 300 400", F1 + " /Properties << /M7 7 0 R >>")
    one("ocg-off-in-form", b"/Fm Do\n", ["SECRET_OCG_IN_FORM"], {6: "<< /Type /OCG /Name (a) >>", 7: "<< /Type /OCG /Name (b) >>", 8: fmo}, cat_extra=ocg,
        resources_extra="/XObject << /Fm 8 0 R >>", hidden=True, regions=[reg(rg(20, 300, "SECRET_OCG_IN_FORM"))], flags={"layers": True})
    # ---- images
    sm_img = {6: img_obj(8, 8, bytes([128] * 192), "/SMask 7 0 R", "/DeviceRGB"), 7: img_obj(8, 8, bytes((i * 37) % 251 for i in range(64)))}
    one("image-smask-only", b"q 80 0 0 80 20 250 cm /Im Do Q\n", [], sm_img, resources_extra="/XObject << /Im 6 0 R >>", regions=[reg([20, 70, 100, 150], "sensitive-image")])
    gray = bytes((i * 13 + j * 7) % 256 for i in range(8) for j in range(8))
    one("image-inline", b"q 80 0 0 80 20 250 cm BI /W 8 /H 8 /CS /G /BPC 8 /F /AHx ID " + gray.hex().encode() + b">\nEI Q\n", [], regions=[reg([20, 70, 100, 150], "sensitive-image", color=[0])])
    pat = stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 100 30] /XStep 100 /YStep 30 /Resources << %s >>" % F1, text(2, 8, "SECRET_PATTERN_CELL", 12))
    one("pattern-text-cell", b"/Pattern cs /P1 scn 20 250 200 60 re f\n", ["SECRET_PATTERN_CELL"], {6: pat}, resources_extra="/Pattern << /P1 6 0 R >>",
        regions=[reg([20, 90, 220, 150], "sensitive-vector")], sanitize_keep=["SECRET_PATTERN_CELL"], expect={"region": "REDACT_PATTERN_UNSUPPORTED", "text": "REDACT_PATTERN_UNSUPPORTED"}, hidden=True)
    pat2 = stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 100 30] /XStep 100 /YStep 30 /Resources << /XObject << /I 7 0 R >> >>", b"q 100 0 0 30 0 0 cm /I Do Q\n")
    one("pattern-image-cell", b"/Pattern cs /P1 scn 20 250 200 60 re f\n", [], {6: pat2, 7: img_obj(8, 8, gray)}, resources_extra="/Pattern << /P1 6 0 R >>",
        regions=[reg([20, 90, 220, 150], "sensitive-vector")], expect={"region": "REDACT_PATTERN_UNSUPPORTED"})
    # ---- fonts
    def t3(cid, secret, unused_marker):
        names = sorted(set(secret))
        codes = {c: 65 + i for i, c in enumerate(names)}
        procs = {"g%d" % codes[c]: stream("", b"700 0 0 0 600 700 d1 50 0 500 700 re f") for c in names}
        procs["gUnused"] = stream("", b"700 0 d0 % " + unused_marker.encode() + b"\n 0 0 100 100 re f")
        ex = {6 + i: p for i, p in enumerate(procs.values())}
        ref = {k: "%d 0 R" % (6 + i) for i, k in enumerate(procs)}
        top = 6 + len(procs)
        bf = " ".join("<%02X> <%04X>" % (codes[c], ord(c)) for c in names)
        ex[top] = stream("", ("/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /T def /CMapType 2 def 1 begincodespacerange <00> <FF> endcodespacerange %d beginbfchar %s endbfchar endcmap end end" % (len(names), bf)).encode())
        lo, hi = min(codes.values()), max(codes.values())
        ex[top + 1] = ("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 700 700] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << %s >> /Encoding << /Type /Encoding /Differences [%s] >> /FirstChar %d /LastChar %d /Widths [%s] /ToUnicode %d 0 R >>"
                       % (" ".join("/%s %s" % kv for kv in ref.items()), " ".join("%d /g%d" % (v, v) for v in sorted(codes.values())), lo, hi,
                          " ".join("700" for _ in range(lo, hi + 1)), top))
        body = b"BT /T3 14 Tf 20 300 Td <" + bytes(codes[c] for c in secret).hex().encode() + b"> Tj ET\n"
        one(cid, body, [secret, unused_marker], ex, fonts="/F1 5 0 R /T3 %d 0 R" % (top + 1), sanitize_keep=[unused_marker], regions=[reg([18, 80, 140, 110])])
    t3("type3-glyphs", "SECRET3ARC", "SECRET_T3_UNUSED_PROC")
    unused = {6: "<< /Type /Font /Subtype /Type1 /BaseFont /SECRET_UNUSED_FONTNAME /FirstChar 32 /LastChar 32 /Widths [250] /ToUnicode 7 0 R /FontDescriptor 8 0 R >>",
              7: stream("", b"%SECRET_UNUSED_TOUNICODE\n/CIDInit /ProcSet findresource begin 12 dict begin begincmap endcmap end end"),
              8: "<< /Type /FontDescriptor /FontName /SECRET_UNUSED_DESCRIPTOR /Flags 32 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 900 /Descent -200 /CapHeight 700 /StemV 80 >>"}
    one("unused-font", text(20, 300, "SECRET_VISIBLE_UNUSEDFONT"), ["SECRET_VISIBLE_UNUSEDFONT", "SECRET_UNUSED_FONTNAME", "SECRET_UNUSED_TOUNICODE", "SECRET_UNUSED_DESCRIPTOR"], unused,
        fonts="/F1 5 0 R /F9 6 0 R", retained=["SECRET_UNUSED_FONTNAME", "SECRET_UNUSED_TOUNICODE", "SECRET_UNUSED_DESCRIPTOR"], regions=[reg(rg(20, 300, "SECRET_VISIBLE_UNUSEDFONT"))])
    ur = {6: form(text(4, 4, "SECRET_UNUSED_FORM", 12), "0 0 200 30"), 7: img_obj(8, 8, gray),
          8: "<< /Type /ExtGState /CA 1 /SECRET_GS_KEY (SECRET_GS_VALUE) >>"}
    one("unused-xobject", text(20, 300, "SECRET_VISIBLE_UNUSEDFORM"), ["SECRET_VISIBLE_UNUSEDFORM", "SECRET_UNUSED_FORM"], ur,
        resources_extra="/XObject << /Unused 6 0 R /UnusedImg 7 0 R >> /ExtGState << /GSunused 8 0 R >>", sanitize_keep=[], regions=[reg(rg(20, 300, "SECRET_VISIBLE_UNUSEDFORM"))],
        note="a Form nobody draws still holds the secret")
    # ---- marked content and stream noise
    bdc = (b"/Span << /Lang (en) /MCID 0 /SECRET_KEY (SECRET_BDC_INLINE) /Alt (SECRET_BDC_ALT) /ActualText (SECRET_BDC_ACT) >> BDC\n" + text(20, 300, "SECRET_MC_TEXT") +
           b"EMC\n/Artifact << /Type /Pagination /Contents (SECRET_ARTIFACT_PROP) >> BDC EMC\n/Tag << /K (SECRET_DP_PROP) >> DP\n/Span /MC9 BDC EMC\n")
    one("marked-props", bdc, ["SECRET_MC_TEXT", "SECRET_BDC_INLINE", "SECRET_BDC_ALT", "SECRET_BDC_ACT", "SECRET_ARTIFACT_PROP", "SECRET_DP_PROP", "SECRET_MC_RES_PROP"],
        resources_extra="/Properties << /MC9 << /Contents (SECRET_MC_RES_PROP) >> >>", sanitize_keep=["SECRET_MC_TEXT"], regions=[reg(rg(20, 300, "SECRET_MC_TEXT"))], flags={"actualtext": True})
    nest = (b"/Span << /ActualText (SECRET_ACT_OUTER) >> BDC /Span << /ActualText (SECRET_ACT_INNER) >> BDC\n" + text(20, 300, "visible words") + b"EMC EMC\n")
    one("actualtext-nested", nest, ["SECRET_ACT_OUTER", "SECRET_ACT_INNER"], hidden=True, regions=[reg(rg(20, 300, "visible words"))], flags={"actualtext": True})
    noise = (b"% SECRET_COMMENT_ONE\nBX /Weird (SECRET_BX_STRING) <</K (SECRET_BX_DICT)>> unknownop EX\n" + text(20, 300, "SECRET_NOISE_TEXT") + b"% SECRET_COMMENT_TWO\n")
    one("content-noise", noise, ["SECRET_COMMENT_ONE", "SECRET_BX_STRING", "SECRET_BX_DICT", "SECRET_NOISE_TEXT", "SECRET_COMMENT_TWO"], sanitize_keep=["SECRET_NOISE_TEXT"],
        regions=[reg(rg(20, 300, "SECRET_NOISE_TEXT"))])
    import base64
    chain = base64.a85encode(zlib.compress(pub("content-chain") + text(20, 300, "SECRET_CHAIN_TEXT"))) + b"~>"
    case("content-chain", simple(b"", {4: ("<< /Filter [/ASCII85Decode /FlateDecode] /Length %d >>\nstream\n" % len(chain)).encode() + chain + b"\nendstream"}), ["SECRET_CHAIN_TEXT"],
         [reg(rg(20, 300, "SECRET_CHAIN_TEXT"))])
    ann = {6: "<< /Type /Annot /Subtype /Stamp /Rect [20 250 220 280] /F 2 /AP << /N 8 0 R >> >>", 7: "<< /Type /Annot /Subtype /Stamp /Rect [20 200 220 230] /F 32 /AP << /N 9 0 R >> >>",
           8: form(text(2, 8, "SECRET_ANNOT_HIDDEN", 12), "0 0 200 30"), 9: form(text(2, 8, "SECRET_ANNOT_NOVIEW", 12), "0 0 200 30")}
    case("annot-hidden", simple(pub("annot-hidden"), ann, page_extra="/Annots [6 0 R 7 0 R]"), ["SECRET_ANNOT_HIDDEN", "SECRET_ANNOT_NOVIEW"], [reg([18, 118, 222, 152])], hidden=True)
    one("rotated-text-30", b"BT /F1 14 Tf 0.866 0.5 -0.5 0.866 40 200 Tm (SECRET_ROTATED) Tj ET\n", ["SECRET_ROTATED"], regions=[reg([36, 134, 140, 204])])
    # ---- document-level hidden data
    xmp = b'<?xpacket begin=""?><x:xmpmeta xmlns:x="adobe:ns:meta/"><dc:title>SECRET_XMP_CATALOG</dc:title></x:xmpmeta><?xpacket end="w"?>'
    ex = {6: "<< /Title (SECRET_INFO_TITLE) /Author (SECRET_INFO_AUTHOR) /SECRET_INFO_KEY (SECRET_INFO_CUSTOM) /Keywords (SECRET_INFO_KEYWORDS) >>",
          7: stream("/Type /Metadata /Subtype /XML", xmp),
          8: "<< /Type /Filespec /F (SECRET_ATT_NAME.txt) /EF << /F 9 0 R >> /Desc (SECRET_ATT_DESC) >>",
          9: stream("/Type /EmbeddedFile", b"SECRET_ATT_BODY", True),
          10: "<< /S /JavaScript /JS (SECRET_JS_DOC) >>", 11: "<< /S /URI /URI (https://example.invalid/SECRET_OPENACTION) >>",
          12: stream("/Type /Metadata /Subtype /XML", xmp.replace(b"CATALOG", b"PAGE")),
          13: "<< /Title (SECRET_OUTLINE_TITLE) /Parent 14 0 R /Dest [3 0 R /Fit] >>", 14: "<< /Type /Outlines /First 13 0 R /Last 13 0 R /Count 1 >>",
          15: "<< /Type /Filespec /F (SECRET_AF_NAME) /EF << /F 9 0 R >> >>",
          16: "<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (SECRET_OUTPUT_INTENT) /Info (SECRET_OI_INFO) >>",
          17: "<< /Type /StructTreeRoot /K 18 0 R /RoleMap << /SECRET_ROLE /P >> >>", 18: "<< /Type /StructElem /S /P /P 17 0 R /Pg 3 0 R /Alt (SECRET_STRUCT_ALT) /ActualText (SECRET_STRUCT_ACT) /E (SECRET_STRUCT_E) /T (SECRET_STRUCT_T) >>",
          19: "<< /S /JavaScript /JS (SECRET_AA_PAGE) >>"}
    cat = ("/Metadata 7 0 R /OpenAction 11 0 R /Outlines 14 0 R /AF [15 0 R] /OutputIntents [16 0 R] /StructTreeRoot 17 0 R /MarkInfo << /Marked true >> "
           "/PieceInfo << /SECRETAPP << /LastModified (D:20200101) /Private << /K (SECRET_PIECE_CAT) >> >> >> /Lang (SECRET_LANG) /SECRET_PRIVATE_CATALOG (SECRET_PRIVATE_CATALOG_VAL) "
           "/PageLabels << /Nums [0 << /P (SECRET_LABEL) >>] >> /Dests << /SECRET_DEST [3 0 R /Fit] >> /Threads [] "
           "/Names << /EmbeddedFiles << /Names [(SECRET_ATT_NAME.txt) 8 0 R] >> /JavaScript << /Names [(n) 10 0 R] >> /Dests << /Names [(SECRET_NAMEDDEST) [3 0 R /Fit]] >> >> "
           "/ViewerPreferences << /SECRET_VP (SECRET_VP_VAL) >> /URI << /Base (https://example.invalid/SECRET_URI_BASE) >>")
    pg = ("/Metadata 12 0 R /AA << /O 19 0 R >> /PieceInfo << /SECRETAPP << /Private (SECRET_PIECE_PAGE) >> >> /LastModified (D:20200102) /SECRET_PRIVATE_PAGE (SECRET_PRIVATE_PAGE_VAL) "
          "/Thumb 20 0 R /StructParents 0 /ID (SECRET_PAGE_ID) /B [] /SeparationInfo << /Pages [3 0 R] /DeviceColorant /SECRET_COLORANT >>")
    ex[20] = stream("/Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8", b"SECRET_THUMB_BYTE"[:16])
    one("hidden-everywhere", text(20, 300, "KEEP_BODY"), ["SECRET_INFO_TITLE", "SECRET_INFO_AUTHOR", "SECRET_INFO_CUSTOM", "SECRET_INFO_KEYWORDS", "SECRET_XMP_CATALOG", "SECRET_XMP_PAGE", "SECRET_ATT_NAME", "SECRET_ATT_DESC",
                                                          "SECRET_ATT_BODY", "SECRET_JS_DOC", "SECRET_OPENACTION", "SECRET_OUTLINE_TITLE", "SECRET_AF_NAME", "SECRET_OUTPUT_INTENT", "SECRET_OI_INFO", "SECRET_STRUCT_ALT", "SECRET_STRUCT_ACT",
                                                          "SECRET_STRUCT_E", "SECRET_STRUCT_T", "SECRET_ROLE", "SECRET_AA_PAGE", "SECRET_PIECE_CAT", "SECRET_PIECE_PAGE", "SECRET_LANG", "SECRET_PRIVATE_CATALOG",
                                                          "SECRET_PRIVATE_PAGE", "SECRET_LABEL", "SECRET_DEST", "SECRET_NAMEDDEST", "SECRET_VP", "SECRET_URI_BASE", "SECRET_PAGE_ID", "SECRET_COLORANT"],
        ex, cat_extra=cat, page_extra=pg, trailer="/Info 6 0 R", keep=["KEEP_PUBLIC_HIDDEN_EVERYWHERE", "KEEP_BODY"], regions=[], hidden=True)
    # ---- file structure
    base, x1 = build({1: "<< /Type /Catalog /Pages 2 0 R >>", 2: "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                      3: "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << %s >> >>" % F1,
                      4: stream("", pub("incremental-info") + text(20, 300, "SECRET_PRIOR_BODY")), 5: "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
                      6: "<< /Title (SECRET_PRIOR_TITLE) >>"}, trailer="/Info 6 0 R")
    pdf = build({4: stream("", pub("incremental-info") + text(20, 300, "KEEP_NEW_BODY")), 6: "<< /Title (new title) >>"}, base=base, prev=x1, trailer="/Info 6 0 R")[0]
    case("incremental-info", pdf, ["SECRET_PRIOR_BODY", "SECRET_PRIOR_TITLE"], [], keep=["KEEP_PUBLIC_INCREMENTAL_INFO", "KEEP_NEW_BODY"], hidden=True)
    orphan = {6: "<< /Orphan (SECRET_ORPHAN_PLAIN) >>"}
    pdf = simple(pub("orphan-trailing") + text(20, 300, "SECRET_ORPHAN_VISIBLE"), orphan)
    pdf += b"\n%SECRET_TRAILING_COMMENT\n99 0 obj\n<< /Length 30 >>\nstream\nSECRET_TRAILING_STREAM_NO_END\n"
    case("orphan-trailing", pdf, ["SECRET_ORPHAN_PLAIN", "SECRET_ORPHAN_VISIBLE", "SECRET_TRAILING_COMMENT", "SECRET_TRAILING_STREAM"], [reg(rg(20, 300, "SECRET_ORPHAN_VISIBLE"))],
         note="orphan object plus trailing garbage after %%EOF; engine may refuse (typed) or drop")
    # object stream with an unreachable compressed object
    objs = {1: "<< /Type /Catalog /Pages 2 0 R >>", 2: "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            3: "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << %s >> >>" % F1,
            4: stream("", pub("objstm-orphan") + text(20, 300, "SECRET_OBJSTM_VISIBLE")), 5: "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"}
    comp = {8: b"<< /Hidden (SECRET_OBJSTM_ORPHAN) >>", 9: b"<< /Other (ok) >>"}
    body = b"".join(b"%d %d " % (n, 0) for n in comp)  # offsets filled below
    offs, pos, data = [], 0, b""
    for n, b in comp.items():
        offs.append((n, pos))
        data += b + b"\n"
        pos = len(data)
    head = b"".join(b"%d %d " % o for o in offs)
    objstm = zlib.compress(head + data)
    out_b = bytearray(HDR)
    off = {}
    for n in sorted(objs):
        off[n] = len(out_b)
        b = objs[n]
        out_b += b"%d 0 obj\n" % n + (b.encode() if isinstance(b, str) else b) + b"\nendobj\n"
    off[7] = len(out_b)
    out_b += b"7 0 obj\n<< /Type /ObjStm /N 2 /First %d /Filter /FlateDecode /Length %d >>\nstream\n" % (len(head), len(objstm)) + objstm + b"\nendstream\nendobj\n"
    xpos = len(out_b)
    rows = b""
    for n in range(0, 11):
        if n == 0:
            rows += struct.pack(">BIH", 0, 0, 65535)
        elif n in off:
            rows += struct.pack(">BIH", 1, off[n], 0)
        elif n in (8, 9):
            rows += struct.pack(">BIH", 2, 7, 0 if n == 8 else 1)
        elif n == 10:
            rows += struct.pack(">BIH", 1, xpos, 0)
        else:
            rows += struct.pack(">BIH", 0, 0, 0)
    xs = zlib.compress(rows)
    out_b += b"10 0 obj\n<< /Type /XRef /Size 11 /W [1 4 2] /Root 1 0 R /Filter /FlateDecode /Length %d >>\nstream\n" % len(xs) + xs + b"\nendstream\nendobj\nstartxref\n%d\n%%%%EOF\n" % xpos
    case("objstm-orphan", bytes(out_b), ["SECRET_OBJSTM_ORPHAN", "SECRET_OBJSTM_VISIBLE"], [reg(rg(20, 300, "SECRET_OBJSTM_VISIBLE"))], note="1.5 object stream, xref stream, unreachable compressed object")
    # ---- geometry
    pdf = build({1: "<< /Type /Catalog /Pages 2 0 R >>", 2: "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                 3: "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /CropBox [100 100 400 500] /Contents 4 0 R /Resources << %s >> >>" % F1,
                 4: stream("", text(120, 450, "KEEP_PUBLIC_CROPBOX") + text(120, 300, "SECRET_INSIDE_CROP") + text(450, 300, "SECRET_OUTSIDE_CROP") + text(120, 50, "SECRET_BELOW_CROP")),
                 5: "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"})[0]
    case("cropbox-outside", pdf, ["SECRET_INSIDE_CROP", "SECRET_OUTSIDE_CROP", "SECRET_BELOW_CROP"], [reg([18, 182, 190, 206])], keep=["KEEP_PUBLIC_CROPBOX"], sanitize_keep=["SECRET_INSIDE_CROP"], hidden=True,
         note="CropBox 100,100,400,500: display coords of the inside secret are x-100, 500-y", flags={"offpage": True})
    pdf = build({1: "<< /Type /Catalog /Pages 2 0 R >>", 2: "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] /Resources << %s >> /Rotate 270 >>" % F1,
                 3: "<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>", 4: stream("", pub("inherited-rot270") + text(40, 200, "SECRET_INHERITED")),
                 5: "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"})[0]
    # Rotate 270 (counter-clockwise 90 for display): point (x,y) -> display (y, x); page displays 400 wide, 300 tall
    case("inherited-rot270", pdf, ["SECRET_INHERITED"], [reg([192, 36, 214, 36 + 0.72 * 14 * len("SECRET_INHERITED") + 4])], note="Resources, MediaBox and Rotate inherited from /Pages")
    pages = {1: "<< /Type /Catalog /Pages 2 0 R >>", 2: "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>", 5: "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
             4: stream("", pub("shared-stream-pages") + text(20, 300, "SECRET_SHARED_STREAM"))}
    for n in (3, 6):
        pages[n] = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << %s >> >>" % F1
    case("shared-stream-pages", build(pages)[0], ["SECRET_SHARED_STREAM"], [reg(rg(20, 300, "SECRET_SHARED_STREAM"), page=1)], survivors=["SECRET_SHARED_STREAM"],
         flags={"keep_page0": "SECRET_SHARED_STREAM"}, note="two pages share one content stream; only page 2 is redacted, page 1 must keep its text")
    pgs = {1: "<< /Type /Catalog /Pages 2 0 R >>", 2: "<< /Type /Pages /Kids [%s] /Count 12 >>" % " ".join("%d 0 R" % (10 + i) for i in range(12)), 5: "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"}
    for i in range(12):
        pgs[10 + i] = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents %d 0 R /Resources << %s >> >>" % (30 + i, F1)
        pgs[30 + i] = stream("", text(20, 370, "KEEP_PUBLIC_P%02d" % i) + text(20, 300, "SECRET_PAGE%02d_BODY" % i) + b"".join(text(20, 200 - 12 * j, "filler line %d of page %d" % (j, i), 10) for j in range(8)), True)
    case("multipage-12", build(pgs)[0], ["SECRET_PAGE%02d_BODY" % i for i in range(12)], [reg(rg(20, 300, "SECRET_PAGE00_BODY"), page=p) for p in range(12)],
         keep=["KEEP_PUBLIC_P%02d" % i for i in range(12)], note="used by the cancellation scenarios")
    for c in CASES:
        (out / (c["id"] + ".pdf")).write_bytes(c.pop("pdf"))
        c["path"] = str(out / (c["id"] + ".pdf"))
    return CASES


if __name__ == "__main__":
    cs = build_all(sys.argv[1])
    json.dump(cs, open(pathlib.Path(sys.argv[1]) / "cases.json", "w"), indent=1)
    print(len(cs), "adversarial fixtures")
