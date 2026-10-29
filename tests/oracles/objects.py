#!/usr/bin/env python3
"""Independent privacy oracle for OUTPUT PDFs (NEW-7). Never imports foliark; uses pypdf + pikepdf + PyMuPDF only.

  .venv/bin/python objects.py out.pdf --forbid SECRET_A --forbid SECRET_B [--allow metadata] [--password pw]
  .venv/bin/python objects.py out.pdf --spec spec.json        # {"forbidden":[], "allow":[], "image_regions":[{"page":0,"rect":[x0,y0,x1,y1],"color":[0,0,0]}], "vector_regions":[{"page":0,"rect":[...],"fill":[0,0,0],"allow":[[...]],"tol":0.25}], "cap":N,
  "occurrences":[{"page":0,"path":[0],"forbid":[],"require":[]}], "no_unused_xobjects":true}

Exit 0 iff no finding. Importable: check_isolated(path, spec) -> [finding strings "CODE: detail"].
Finding codes: OPEN TEXT_PYPDF TEXT_FITZ TEXT_GLYPH BYTES_RAW STREAM STRING REVISION ORPHAN DANGLING FEATURE DECODE UNSUPPORTED CAP IMAGE_REGION VECTOR_REGION OCCURRENCE UNUSED_XOBJECT TIMEOUT CRASH.
Every decode problem, unsupported filter or unsupported geometry is a finding, never a silent pass.
"""
import json
import math
import re
import unicodedata
import urllib.parse
import subprocess
import sys
import zlib

import fitz
import pikepdf
from pikepdf import ObjectType as T
from pypdf import PdfReader

CAP = 64 << 20  # decoded bytes per stream
GENERIC = {"/FlateDecode", "/LZWDecode", "/ASCII85Decode", "/ASCIIHexDecode", "/RunLengthDecode"}
CODECS = {"/DCTDecode", "/JPXDecode", "/CCITTFaxDecode", "/JBIG2Decode"}  # image codecs: bytes kept, pixels checked via MuPDF
FEATURES = {"/Metadata": "metadata", "/EmbeddedFiles": "embeddedfiles", "/EmbeddedFile": "embeddedfiles", "/EF": "embeddedfiles",
            "/AF": "embeddedfiles", "/JavaScript": "javascript", "/JS": "javascript", "/OpenAction": "openaction", "/AA": "aa",
            "/AcroForm": "acroform", "/XFA": "acroform", "/OCProperties": "ocproperties", "/Encrypt": "encrypt"}


def needles(s):
    return [s.encode(), s.encode("utf-16-be"), s.encode("utf-16-le")]


def decode(s, cap):
    """Decoded bytes of a stream (generalized filters), or raise ValueError('CODE: ...')."""
    flt = s.get("/Filter")
    names = [] if flt is None else [str(f) for f in (flt if flt._type_code == T.array else [flt])]
    bad = [n for n in names if n not in GENERIC | CODECS]
    if bad:
        raise ValueError("UNSUPPORTED: filter %s" % bad)
    raw = s.read_raw_bytes()
    if not names or all(n in CODECS for n in names):  # an image codec stream: its bytes are the payload (pixels are judged via MuPDF)
        return raw
    plain_flate = names == ["/FlateDecode"] and "/Predictor" not in str(s.get("/DecodeParms", ""))
    if plain_flate:
        d = zlib.decompressobj()
        try:
            out = d.decompress(raw, cap + 1)
        except zlib.error as e:
            raise ValueError("DECODE: flate %s" % e)
        if len(out) > cap:
            raise ValueError("CAP: decoded > %d bytes" % cap)
        if not d.eof:
            raise ValueError("DECODE: truncated flate stream")
        return out
    if len(raw) > cap:
        raise ValueError("CAP: encoded > %d bytes" % cap)
    try:
        out = s.read_bytes(pikepdf.StreamDecodeLevel.generalized)
    except Exception as e:
        raise ValueError("DECODE: %s" % e)
    if len(out) > cap:
        raise ValueError("CAP: decoded > %d bytes" % cap)
    return out


def shown_text(s):
    """All string operands of Tj/TJ/'/" in a content stream, concatenated (b'' if it does not parse as content)."""
    out = b""
    try:
        for ins in pikepdf.parse_content_stream(s):
            if str(ins.operator) in ("Tj", "'", '"'):
                out += bytes(ins.operands[-1])
            elif str(ins.operator) == "TJ":
                out += b"".join(bytes(x) for x in ins.operands[0] if isinstance(x, pikepdf.Object) and x._type_code == T.string)
    except Exception:
        pass
    return out


def program_names(doc, xref):
    """Glyph id -> lowest code point of an embedded TrueType/OpenType program's own cmap (FreeType via PyMuPDF), {} if none."""
    try:
        _, ext, _, buf = doc.extract_font(xref)
        font = fitz.Font(fontbuffer=buf) if ext in ("ttf", "otf") and buf else None
    except Exception:
        return {}
    inv = {}
    for cp in font.valid_codepoints() if font else []:
        g = font.has_glyph(cp)
        if g and cp >= 0x20:
            inv.setdefault(g, cp)
    return inv


def glyph_text(doc):
    """Per page, two readings of every shown glyph (MuPDF's trace) named by its font program's own cmap: pypdf and PyMuPDF
    read a font whose ToUnicode maps nothing (iOS Quartz Type0) as junk, so its glyphs would pass both extractors unseen.
    A glyph the program does not name keeps MuPDF's Unicode; fonts sharing a name (Word's TrueType and Type0 twins) are
    read with the first and with the last of them."""
    names, pages = {}, []
    for pg in doc:
        by_name = {}
        for f in pg.get_fonts(full=True):
            by_name.setdefault(re.sub(r"^[A-Z]{6}\+", "", f[3]), []).append(f[0])
        first, last = [], []
        for span in pg.get_texttrace():
            maps = [names.setdefault(x, program_names(doc, x)) for x in by_name.get(span["font"], [])] or [{}]
            for out, m in ((first, maps[0]), (last, maps[-1])):
                out.append("".join(chr(m.get(g, u)) for u, g, *_ in span["chars"]))
        pages.append(("".join(first), "".join(last)))
    return pages


def named_copy(path, out):
    """`path` with every Type0 Identity-H font whose ToUnicode maps nothing (iOS Quartz; glyph id = CID: no CIDToGIDMap or
    /Identity) given a ToUnicode from its program's own cmap, written to `out`; returns the path to judge against (`path`
    itself when no font needed it). PyMuPDF and pypdf read such a font as junk; an export whose ToUnicode names the glyphs is
    compared with the input as its glyphs read. Other fonts and every byte that is drawn stay as they are."""
    doc, pdf, named = fitz.open(path), pikepdf.open(path), False
    for f in pdf.objects:
        if f is None or f._type_code != T.dictionary or f.get("/Subtype") != "/Type0" or f.get("/Encoding") != "/Identity-H":
            continue
        tu, kids = f.get("/ToUnicode"), f.get("/DescendantFonts")
        if tu is not None and re.search(rb"begin(bfchar|bfrange)", tu.read_bytes()):
            continue
        if not kids or str(kids[0].get("/CIDToGIDMap", "/Identity")) != "/Identity" or not f.is_indirect:
            continue
        inv = program_names(doc, f.objgen[0])
        if not inv:
            continue
        pairs = sorted(inv.items())
        body = "".join("%d beginbfchar\n%sendbfchar\n" % (len(c), "".join("<%04X> <%s>\n" % (g, chr(u).encode("utf-16-be").hex().upper()) for g, u in c))
                       for c in (pairs[i:i + 100] for i in range(0, len(pairs), 100)))
        f.ToUnicode = pdf.make_stream(("/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Named def /CMapType 2 def\n"
                                       "1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n%sendcmap CMapName currentdict /CMap "
                                       "defineresource pop end end\n" % body).encode())
        named = True
    if not named:
        return path
    pdf.save(out)
    return out


def _res(o):
    return (o.obj if isinstance(o, pikepdf.Page) else o).get("/Resources")


def _draws(o):
    """(Form XObjects drawn by `Do` in the content of page/Form `o`, in order; every Do name; the XObject map)."""
    xo = _res(o)
    xo = xo.get("/XObject") if xo is not None else None
    xo = {str(k): v for k, v in xo.items()} if xo is not None else {}
    forms, names = [], []
    for ins in pikepdf.parse_content_stream(o):
        if str(ins.operator) == "Do":
            n = str(ins.operands[0])
            names.append(n)
            x = xo.get(n)
            if x is not None and str(x.get("/Subtype", "")) == "/Form":
                forms.append(x)
    return forms, names, xo


def _reach(form, cap, seen=()):
    """Decoded bytes and shown text of `form` and of every Form its content draws, at any depth."""
    data = decode(form, cap) + b"\0" + shown_text(form)
    for sub in _draws(form)[0]:
        if sub.objgen not in seen:
            data += b"\0" + _reach(sub, cap, seen + (form.objgen,))
    return data


def occurrence_leaks(pdf, r, cap):
    """The Form reached by `path` (indices of the n-th Form drawn by the page, then by that Form, ...): nothing it
    reaches (nested Forms included) may hold a `forbid` string, everything it reaches must hold each `require`."""
    path = r["path"]
    where = "form draw %s of page %d" % (path, r["page"])
    node = pdf.pages[r["page"]]
    for i in path:
        forms = _draws(node)[0]
        if i >= len(forms):
            return ["OCCURRENCE: %s does not exist (%d form draws)" % (where, len(forms))]
        node = forms[i]
    try:
        data = _reach(node, cap)
    except ValueError as e:
        return [str(e)]
    hit = lambda s: any(n in data for n in needles(s))
    return ["OCCURRENCE: %r reachable from %s" % (s, where) for s in r.get("forbid", []) if hit(s)] + [
        "OCCURRENCE: %r missing from %s" % (s, where) for s in r.get("require", []) if not hit(s)]


def unused_xobjects(pdf):
    """Every XObject resource of a page or a drawn Form is drawn by that owner's own content: an original that
    every occurrence was redirected away from must not stay in the resources."""
    F, seen = [], set()

    def walk(o, label):
        forms, names, xo = _draws(o)
        F.extend("UNUSED_XOBJECT: %s in %s is not drawn" % (n, label) for n in xo if n not in names)
        for n in set(names):
            x = xo.get(n)
            if x is not None and str(x.get("/Subtype", "")) == "/Form" and x.objgen not in seen:
                seen.add(x.objgen)
                walk(x, "form %s" % (x.objgen,))

    for i, pg in enumerate(pdf.pages):
        walk(pg, "page %d" % i)
    return F


def children(o):
    t = o._type_code
    if t in (T.dictionary, T.stream):
        kv = list(o.items())
    elif t == T.array:
        kv = [(None, v) for v in o]
    else:
        return []
    return [(k, v) for k, v in kv if isinstance(v, pikepdf.Object)]  # scalars (int/bool/float) come back as Python types


def check(path, spec):
    F = []
    forb = spec.get("forbidden", [])
    allow = set(spec.get("allow", []))
    cap = spec.get("cap", CAP)
    raw = open(path, "rb").read()
    try:
        pdf = pikepdf.open(path, password=spec.get("password", ""), attempt_recovery=False)
    except Exception as e:
        return ["OPEN: %s" % e]
    # (c) revisions
    if raw.count(b"%%EOF") != 1 or raw.count(b"startxref") != 1:
        F.append("REVISION: %d %%EOF / %d startxref markers (want 1/1)" % (raw.count(b"%%EOF"), raw.count(b"startxref")))
    if re.search(rb"/Prev\s+\d+", raw):
        F.append("REVISION: /Prev in file")
    heads = [(int(a), int(b)) for a, b in re.findall(rb"(?m)^(\d+) (\d+) obj\b", raw)]
    objs = [o for o in pdf.objects if o is not None]
    allobj = {o.objgen for o in objs} | set(heads)  # a dangling target comes back as None
    if len(heads) != len(set(heads)):
        F.append("REVISION: duplicate object definitions (prior revision bytes)")
    for g in set(heads) - allobj:
        F.append("ORPHAN: raw object %s not in xref" % (g,))
    seen, stack = set(), [pdf.trailer]
    while stack:
        o = stack.pop()
        if o.is_indirect:
            if o.objgen in seen:
                continue
            seen.add(o.objgen)
        stack.extend(v for _, v in children(o))
    # qpdf drops null-valued keys and materializes the target of a reference nothing defines as a null object, so an
    # undefined target shows as more null objects than the file's own `null` definitions.
    undefined = len(pdf.objects) - len(objs) - len(re.findall(rb"(?m)^\d+ \d+ obj\s+null\b", raw))
    if undefined > 0:
        F.append("DANGLING: %d reference(s) to an object the file does not define" % undefined)
    for o in objs:
        if o.objgen not in seen and not (o._type_code == T.stream and str(o.get("/Type", "")) in ("/ObjStm", "/XRef")):
            F.append("ORPHAN: unreachable object %s" % (o.objgen,))
    # (b) forbidden content in raw bytes, every decoded stream, every string/name
    for s in forb:
        if any(n in raw for n in needles(s)):
            F.append("BYTES_RAW: %r in file bytes" % s)
    if pdf.is_encrypted or "/Encrypt" in pdf.trailer:
        feats0 = {"encrypt": "trailer /Encrypt"}
    else:
        feats0 = {}
    info = pdf.trailer.get("/Info")
    if info is not None and len(info.keys()) and "info" not in allow:
        F.append("FEATURE: /Info present (%s)" % ",".join(info.keys()))
    feats = dict(feats0)
    for o in objs:
        t = o._type_code
        if t not in (T.dictionary, T.stream, T.array):
            continue
        work = [o]
        while work:
            c = work.pop()
            for k, v in children(c):
                if k is not None:
                    if k in FEATURES:
                        feats.setdefault(FEATURES[k], "%s in object %s" % (k, o.objgen))
                    blob = k.encode()
                    for s in forb:
                        if s.encode() in blob:
                            F.append("STRING: %r in key %s of %s" % (s, k, o.objgen))
                vt = v._type_code
                if vt in (T.string, T.name):
                    b = bytes(v) if vt == T.string else str(v).encode()
                    for s in forb:
                        if any(n in b for n in needles(s)):
                            F.append("STRING: %r in object %s" % (s, o.objgen))
                elif vt in (T.dictionary, T.array, T.stream) and not v.is_indirect:
                    work.append(v)
        if t == T.stream:
            if str(o.get("/Type", "")) == "/Metadata":
                feats.setdefault("metadata", "XMP stream %s" % (o.objgen,))
            if str(o.get("/Type", "")) in ("/EmbeddedFile",):
                feats.setdefault("embeddedfiles", "stream %s" % (o.objgen,))
            try:
                data = decode(o, cap)
            except ValueError as e:
                F.append("%s (object %s)" % (e, o.objgen))
                continue
            if forb and (str(o.get("/Subtype", "")) == "/Form" or "/Type" not in o):
                data += b"\0" + shown_text(o)  # text operands: hex strings / TJ pieces never appear as plain bytes
            for s in forb:
                if any(n in data for n in needles(s)):
                    F.append("STREAM: %r in decoded stream %s" % (s, o.objgen))
    # (d) features
    for f, where in feats.items():
        if f not in allow:
            F.append("FEATURE: %s (%s)" % (f, where))
    # (a) two independent text extractors
    squash = lambda x: re.sub(r"\s+", "", x)
    try:
        rd = PdfReader(path, strict=False, password=spec.get("password") or None)
        for i, pg in enumerate(rd.pages):
            tx = squash(pg.extract_text() or "")
            F += ["TEXT_PYPDF: %r on page %d" % (s, i) for s in forb if squash(s) in tx]
    except Exception as e:
        F.append("TEXT_PYPDF: extractor failed: %s" % e)
    try:
        doc = fitz.open(path)
        for i, pg in enumerate(doc):
            tx = squash(pg.get_text("text", flags=0))
            F += ["TEXT_FITZ: %r on page %d" % (s, i) for s in forb if squash(s) in tx]
        for i, readings in enumerate(glyph_text(doc) if forb else []):
            F += ["TEXT_GLYPH: %r on page %d" % (s, i) for s in forb if any(squash(s) in squash(t) for t in readings)]
    except Exception as e:
        F.append("TEXT_FITZ: extractor failed: %s" % e)
        return F
    # (e) image pixels inside display-space rects
    for r in spec.get("image_regions", []):
        F += image_region(doc, r)
    # (f) vector paths inside display-space rects
    for r in spec.get("vector_regions", []):
        F += vector_region(doc, r)
    if spec.get("links"):
        F += link_check(pdf, doc, spec)
    # (g) per-occurrence reachability and stale resources (Form clones)
    for r in spec.get("occurrences", []):
        F += occurrence_leaks(pdf, r, cap)
    if spec.get("no_unused_xobjects"):
        F += unused_xobjects(pdf)
    return F


def grow(rc, e):
    return fitz.Rect(rc.x0 - e, rc.y0 - e, rc.x1 + e, rc.y1 + e)


def unrot(pg, rect):
    """Display-space rect (spec §2) -> PyMuPDF's unrotated page space, where get_image_info/get_drawings report geometry."""
    return (fitz.Rect(rect) * ~pg.rotation_matrix).normalize()


def pix_spans(m, reg, w, h):
    """Yield (row, i0, i1) pixel spans of a w x h image placed by matrix m (unit square, row 0 at v=0) that fall under reg.
    Axis-aligned placements (any 90 degree turn or flip): every pixel square intersecting reg. Other angles: pixel centres inside reg."""
    inv, eps = ~m, 1e-6
    pts = [fitz.Point(x, y) * inv for x, y in ((reg.x0, reg.y0), (reg.x1, reg.y0), (reg.x0, reg.y1), (reg.x1, reg.y1))]
    u0, u1, v0, v1 = min(p.x for p in pts), max(p.x for p in pts), min(p.y for p in pts), max(p.y for p in pts)
    i0, i1, j0, j1 = max(0, math.floor(u0 * w + eps)), min(w, math.ceil(u1 * w - eps)), max(0, math.floor(v0 * h + eps)), min(h, math.ceil(v1 * h - eps))
    axis = (abs(m.b) < 1e-3 and abs(m.c) < 1e-3) or (abs(m.a) < 1e-3 and abs(m.d) < 1e-3)
    if axis:
        for j in range(j0, j1):
            yield j, i0, i1
        return
    for j in range(j0, j1):
        xs = [i for i in range(i0, i1) if reg.contains(fitz.Point((i + .5) / w, (j + .5) / h) * m)]
        if xs:
            yield j, xs[0], xs[-1] + 1


def region_colors(pix, m, reg, cap=8_000_000):
    """Distinct pixel tuples of pix under reg (placement m). Returns (set, pixel_count)."""
    pix = pix if pix.colorspace is not None and pix.colorspace.n in (1, 3) and not pix.alpha else fitz.Pixmap(fitz.csRGB, pix)
    n, stride, s, seen, count = pix.n, pix.stride, pix.samples, set(), 0
    for j, i0, i1 in pix_spans(m, reg, pix.width, pix.height):
        count += i1 - i0
        if count > cap:
            raise ValueError("CAP: image region > %d pixels" % cap)
        row = s[j * stride + i0 * n:j * stride + i1 * n]
        if row and row == row[:n] * (len(row) // n):
            seen.add(tuple(row[:n]))
        else:
            seen.update(tuple(row[k:k + n]) for k in range(0, len(row), n))
    return seen, count


def plane_pixmap(doc, x):
    """Samples of image xref x. A 1-bit /ImageMask stencil has no colour space (PyMuPDF refuses it), so it is unpacked to gray by hand."""
    if doc.xref_get_key(x, "ImageMask")[1] != "true":
        return fitz.Pixmap(doc, x)
    w, h = int(doc.xref_get_key(x, "Width")[1]), int(doc.xref_get_key(x, "Height")[1])
    data, row = doc.xref_stream(x), (w + 7) // 8
    return fitz.Pixmap(fitz.csGRAY, w, h, bytes(255 * (data[j * row + i // 8] >> (7 - i % 8) & 1) for j in range(h) for i in range(w)), False)


def image_users(doc, xref, cache={}):
    """Pages (0-based) whose occurrences use xref; computed lazily, only to explain a failure."""
    key = id(doc)
    if key not in cache:
        cache.clear()
        cache[key] = [{im["xref"] for im in pg.get_image_info(xrefs=True)} for pg in doc]
    return [i for i, xs in enumerate(cache[key]) if xref in xs]


def image_region(doc, r):
    """Every image occurrence (page content, Form XObjects, any CTM, any page /Rotate) intersecting the display-space region:
    colour samples AND SMask/Mask samples under it must be one colour (optionally exactly r["color"]). One finding per occurrence."""
    pg = doc[r["page"]]
    reg, F, hit = unrot(pg, r["rect"]), [], 0
    for k, im in enumerate(pg.get_image_info(xrefs=True)):
        if not fitz.Rect(im["bbox"]).intersects(reg):
            continue
        hit += 1
        m, x = fitz.Matrix(im["transform"]), im["xref"]
        who = "page %d occurrence %d" % (r["page"], k)
        if x <= 0:
            F.append("UNSUPPORTED: %s is an inline image intersecting the region" % who)
            continue
        try:
            keys = doc.xref_get_keys(x)
            if "SMaskInData" in keys and doc.xref_get_key(x, "SMaskInData")[1] != "0":
                F.append("UNSUPPORTED: %s image %d has SMaskInData (mask lives in the codestream)" % (who, x))
            planes = [("colour", x, tuple(r["color"]) if r.get("color") else None)]
            for key in ("SMask", "Mask"):
                t, v = doc.xref_get_key(x, key) if key in keys else ("null", "")
                if t == "xref":
                    planes.append((key, int(v.split()[0]), None))
            bad = []
            for name, px, want in planes:
                seen, cnt = region_colors(plane_pixmap(doc, px), m, reg)
                if not cnt:
                    bad.append("%s: no sample under region (placement not mapped)" % name)
                elif len(seen) != 1 or (want and seen != {want}):
                    bad.append("%s: %d distinct%s" % (name, len(seen), "" if len(seen) != 1 else " (%s != %s)" % (seen, want)))
            if bad:
                sh = [p for p in image_users(doc, x) if p != r["page"]]
                F.append("IMAGE_REGION: %s image %d%s region %s: %s" % (
                    who, x, " (shared, also used on pages %s: a surviving occurrence still holds these samples)" % sh if sh else "", r["rect"], "; ".join(bad)))
        except ValueError as e:
            F.append("%s (%s image %d)" % (e, who, x))
        except Exception as e:
            F.append("DECODE: %s image %d: %s" % (who, x, e))
    if not hit:
        F.append("IMAGE_REGION: no image intersects %s (expected one)" % r["rect"])
    return F


def seg_hits(p, q, rc):
    """Segment p-q intersects rectangle rc (Liang-Barsky)."""
    t0, t1, dx, dy = 0.0, 1.0, q[0] - p[0], q[1] - p[1]
    for d, lo, hi, o in ((dx, rc.x0 - p[0], rc.x1 - p[0], 0), (dy, rc.y0 - p[1], rc.y1 - p[1], 1)):
        if abs(d) < 1e-12:
            if lo > 0 or hi < 0:
                return False
            continue
        a, b = sorted((lo / d, hi / d))
        t0, t1 = max(t0, a), min(t1, b)
        if t0 > t1:
            return False
    return True


def polys(items):
    """Flatten PyMuPDF drawing items to polylines (curves sampled at 12 steps)."""
    out, cur = [], []
    for it in items:
        if it[0] in ("re", "qu"):
            q = it[1].quad if it[0] == "re" else it[1]
            out.append([(q.ul.x, q.ul.y), (q.ur.x, q.ur.y), (q.lr.x, q.lr.y), (q.ll.x, q.ll.y), (q.ul.x, q.ul.y)])
            continue
        pts = [(it[1].x, it[1].y)]
        if it[0] == "l":
            pts.append((it[2].x, it[2].y))
        else:
            p0, p1, p2, p3 = it[1:5]
            pts += [((1 - t) ** 3 * p0.x + 3 * (1 - t) ** 2 * t * p1.x + 3 * (1 - t) * t * t * p2.x + t ** 3 * p3.x,
                     (1 - t) ** 3 * p0.y + 3 * (1 - t) ** 2 * t * p1.y + 3 * (1 - t) * t * t * p2.y + t ** 3 * p3.y) for t in [k / 12 for k in range(1, 13)]]
        if cur and math.dist(cur[-1], pts[0]) < 1e-6:
            cur += pts[1:]
        else:
            if cur:
                out.append(cur)
            cur = pts
    if cur:
        out.append(cur)
    return out


def inside(poly, pt):
    c, (x, y) = False, pt
    for (x0, y0), (x1, y1) in zip(poly, poly[1:] + poly[:1]):
        if (y0 > y) != (y1 > y) and x < (x1 - x0) * (y - y0) / (y1 - y0) + x0:
            c = not c
    return c


def filled(pl, pt, even_odd):
    """Is pt inked by the contours pl under the even-odd or the nonzero rule?"""
    if even_odd:
        return sum(inside(poly, pt) for poly in pl) % 2 == 1
    w, (x, y) = 0, pt
    for poly in pl:
        for (x0, y0), (x1, y1) in zip(poly, poly[1:] + poly[:1]):
            side = (x1 - x0) * (y - y0) - (x - x0) * (y1 - y0)
            if y0 <= y < y1 and side > 0:
                w += 1
            elif y1 <= y < y0 and side < 0:
                w -= 1
    return w != 0


def path_hits(d, reg, tol):
    """Does painted geometry of drawing d reach into reg (shrunk by tol)? Fills: polygon vs rect. Strokes: segments vs rect grown by half width."""
    core = grow(reg, -tol)
    if core.is_empty:
        core = reg
    pl = polys(d["items"])
    if d.get("fill") is not None and "f" in d["type"]:
        for poly in pl:
            if any(core.contains(fitz.Point(p)) for p in poly) or any(seg_hits(a, b, core) for a, b in zip(poly, poly[1:] + poly[:1])):
                return True
        # No edge reaches the region: it is inked only if it lies inside the fill. The rule acts across all contours of
        # the path (a hole winds against its outer), so judge the contours together, not one by one.
        # get_drawings reports a four-sided subpath as a `re`/`qu` item and drops its winding direction, so a nonzero path
        # holding one is judged by parity (a hole winds against its outer); vector_cut.py judges the true direction.
        eo = d.get("even_odd") or any(it[0] in ("re", "qu") for it in d["items"])
        for c in ((core.x0, core.y0), (core.x1, core.y0), (core.x0, core.y1), (core.x1, core.y1)):
            if filled(pl, c, eo):
                return True
    if d.get("color") is not None and "s" in d["type"]:
        g = grow(core, d["width"] / 2)
        if any(seg_hits(a, b, g) for poly in pl for a, b in zip(poly, poly[1:])):
            return True
    return False


def vector_region(doc, r):
    """Vector paths (page content and Form XObjects) whose painted geometry remains inside the display-space region.
    Allowed: the declared redaction fill (r["fill"], 0-255 RGB) when it is the LAST painted hit and covers the region, and any path
    whose bounds lie inside a declared r["allow"] rect (collateral). Everything else is a finding. Clips are not evaluated."""
    pg = doc[r["page"]]
    reg, tol = unrot(pg, r["rect"]), r.get("tol", 0.25)
    allow = [unrot(pg, a) for a in r.get("allow", [])]
    hits = [d for d in pg.get_drawings() if path_hits(d, reg, tol)]
    last = max(hits, key=lambda d: d["seqno"], default=None)
    fill = tuple(r["fill"]) if r.get("fill") else None
    F = []
    for d in hits:
        if any(grow(a, .5).contains(d["rect"]) for a in allow):
            continue
        col = d.get("fill") and tuple(round(c * 255) for c in d["fill"])
        if d is last and fill and col == fill and d["rect"].contains(grow(reg, -.5)):
            continue
        F.append("VECTOR_REGION: page %d path #%d %s bounds %s fill %s stroke %s remains inside region %s" % (
            r["page"], d["seqno"], d["type"], tuple(round(v, 2) for v in d["rect"]), d.get("fill"), d.get("color"), r["rect"]))
    return F


LINK_KEYS = {"/Type", "/Subtype", "/Rect", "/QuadPoints", "/Border", "/H", "/A", "/Dest"}
LINK_SCHEMES = ("http:", "https:", "mailto:")
LINK_VIEWS = {"/XYZ", "/Fit", "/FitH", "/FitV", "/FitR", "/FitB", "/FitBH", "/FitBV"}


def _norm(s):
    return unicodedata.normalize("NFC", s).lower()


def link_check(pdf, doc, spec):
    """ADR 0014. Every annotation of the output is a /Link written from the allowlist: only allowlisted keys, an /A that is
    exactly a URI action (http, https, mailto; printable ASCII, 2048 bytes at most) or an explicit /Dest naming a page of this
    file, no URI holding a token of the redacted text (spec["links"]["tokens"], percent-decoded, NFC, lowercased), and no
    /Rect or quad meeting a redaction region (spec["links"]["regions"], display space)."""
    L, F = spec["links"], []
    pages = {pg.obj.objgen for pg in pdf.pages}
    toks = [_norm(t) for t in L.get("tokens", [])]
    for i, pg in enumerate(pdf.pages):
        fpg = doc[i]
        regs = [unrot(fpg, r["rect"]) for r in L.get("regions", []) if r["page"] == i]
        for n, a in enumerate(pg.obj.get("/Annots", [])):
            where = "page %d annotation %d" % (i, n)
            if str(a.get("/Subtype", "")) != "/Link":
                F.append("LINK_SUBTYPE: %s is a %s, not a /Link" % (where, a.get("/Subtype")))
                continue
            extra = sorted(set(str(k) for k in a.keys()) - LINK_KEYS)
            if extra:
                F.append("LINK_KEY: %s carries %s" % (where, ",".join(extra)))
            act = a.get("/A")
            if act is not None:
                if sorted(str(k) for k in act.keys()) != ["/S", "/URI"] or str(act.get("/S")) != "/URI":
                    F.append("LINK_ACTION: %s action is not a plain URI action (%s)" % (where, ",".join(str(k) for k in act.keys())))
                    continue
                raw = bytes(act["/URI"])
                if len(raw) > 2048 or not all(0x21 <= c <= 0x7E for c in raw) or not raw.lower().startswith(tuple(x.encode() for x in LINK_SCHEMES)):
                    F.append("LINK_SCHEME: %s URI %r is not a printable http, https or mailto URI" % (where, raw[:60]))
                dec = raw.decode("latin-1")
                for _ in range(3):
                    dec = urllib.parse.unquote(dec, errors="replace")
                for t in toks:
                    if t in _norm(dec):
                        F.append("LINK_TOKEN: %s URI holds redacted text %r" % (where, t))
            elif "/Dest" in a:
                d = a["/Dest"]
                ok = d._type_code == T.array and len(d) >= 2 and d[0].is_indirect and d[0].objgen in pages and str(d[1]) in LINK_VIEWS
                if not ok:
                    F.append("LINK_DEST: %s /Dest is not an explicit destination on a page of this file" % where)
            else:
                F.append("LINK_ACTION: %s has neither /A nor /Dest" % where)
            shapes = [[float(v) for v in a["/Rect"]]]
            qp = [float(v) for v in a.get("/QuadPoints", [])]
            shapes += [qp[k:k + 8] for k in range(0, len(qp) - len(qp) % 8, 8)]
            for sh in shapes:
                xs, ys = sh[0::2], sh[1::2]
                r = (fitz.Rect(min(xs), min(ys), max(xs), max(ys)) * fpg.transformation_matrix).normalize()
                for g in regs:
                    if r.x0 < g.x1 and g.x0 < r.x1 and r.y0 < g.y1 and g.y0 < r.y1:
                        F.append("LINK_REGION: %s overlaps redaction region %s" % (where, tuple(round(v, 1) for v in g)))
    return F


def check_isolated(path, spec, timeout=120, script=__file__):
    try:
        p = subprocess.run([sys.executable, script, "--worker"], input=json.dumps({"path": str(path), "spec": spec}), capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return ["TIMEOUT: oracle exceeded %ds" % timeout]
    if p.returncode != 0:
        return ["CRASH: %s" % (p.stderr.strip()[-300:] or p.returncode)]
    if "\n@@" not in p.stdout:
        return ["CRASH: no result line from worker"]
    return json.loads(p.stdout.rsplit("\n@@", 1)[1])


def main(a):
    if a[:1] == ["--worker"]:
        j = json.load(sys.stdin)
        res = check(j["path"], j["spec"])
        sys.stdout.flush()
        print("\n@@" + json.dumps(res))  # MuPDF may write warnings to stdout; the parent reads only the marked line
        return 0
    if not a or a[0].startswith("-"):
        print(__doc__)
        return 2
    spec = {"forbidden": [], "allow": []}
    i = 1
    while i < len(a):
        if a[i] == "--spec":
            spec.update(json.load(open(a[i + 1])))
        elif a[i] == "--forbid":
            spec["forbidden"].append(a[i + 1])
        elif a[i] == "--allow":
            spec["allow"].append(a[i + 1])
        elif a[i] == "--password":
            spec["password"] = a[i + 1]
        else:
            print("unknown arg", a[i])
            return 2
        i += 2
    f = check_isolated(a[0], spec)
    for x in f:
        print("FAIL", x)
    print("%s: %d findings" % (a[0], len(f)))
    return 1 if f else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
