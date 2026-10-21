#!/usr/bin/env python3
"""Independent privacy oracle for OUTPUT PDFs (NEW-7). Never imports foliark; uses pypdf + pikepdf + PyMuPDF only.

  .venv/bin/python objects.py out.pdf --forbid SECRET_A --forbid SECRET_B [--allow metadata] [--password pw]
  .venv/bin/python objects.py out.pdf --spec spec.json        # {"forbidden":[], "allow":[], "image_regions":[{"page":0,"rect":[x0,y0,x1,y1],"color":[0,0,0]}], "cap":N}

Exit 0 iff no finding. Importable: check_isolated(path, spec) -> [finding strings "CODE: detail"].
Finding codes: OPEN TEXT_PYPDF TEXT_FITZ BYTES_RAW STREAM STRING REVISION ORPHAN FEATURE DECODE UNSUPPORTED CAP IMAGE_REGION TIMEOUT CRASH.
Every decode problem, unsupported filter or unsupported geometry is a finding, never a silent pass.
"""
import json
import math
import re
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
    if not names:
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
    allobj = {o.objgen for o in pdf.objects}
    heads = [(int(a), int(b)) for a, b in re.findall(rb"(?m)^(\d+) (\d+) obj\b", raw)]
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
    for o in pdf.objects:
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
    for o in pdf.objects:
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
    except Exception as e:
        F.append("TEXT_FITZ: extractor failed: %s" % e)
        return F
    # (e) image pixels inside display-space rects
    for r in spec.get("image_regions", []):
        F += image_region(doc, r)
    return F


def image_region(doc, r):
    pg = doc[r["page"]]
    if pg.rotation:
        return ["UNSUPPORTED: image check on rotated page %d" % r["page"]]
    reg, F, hit = fitz.Rect(r["rect"]), [], 0
    for im in pg.get_image_info(xrefs=True):
        bb = fitz.Rect(im["bbox"])
        if not bb.intersects(reg):
            continue
        hit += 1
        m = fitz.Matrix(im["transform"])
        if abs(m.b) > 1e-3 or abs(m.c) > 1e-3 or m.a <= 0 or m.d <= 0:
            F.append("UNSUPPORTED: rotated/flipped image transform %s" % (tuple(m),))
            continue
        x = im["xref"]
        if x <= 0:
            F.append("UNSUPPORTED: inline image intersects region")
            continue
        try:
            if any(k in doc.xref_get_keys(x) for k in ("SMask", "Mask")):
                F.append("UNSUPPORTED: image %d has SMask/Mask (mask samples not checked)" % x)
            pix = fitz.Pixmap(doc, x)
            if pix.colorspace is None or pix.colorspace.n != 3 or pix.alpha:
                pix = fitz.Pixmap(fitz.csRGB, pix)
        except Exception as e:
            F.append("DECODE: image %d: %s" % (x, e))
            continue
        w, h = pix.width, pix.height
        x0, x1 = math.floor((max(reg.x0, bb.x0) - bb.x0) / bb.width * w), math.ceil((min(reg.x1, bb.x1) - bb.x0) / bb.width * w)
        y0, y1 = math.floor((max(reg.y0, bb.y0) - bb.y0) / bb.height * h), math.ceil((min(reg.y1, bb.y1) - bb.y0) / bb.height * h)
        px = {tuple(pix.pixel(i, j)) for j in range(y0, min(y1, h)) for i in range(x0, min(x1, w))}
        want = tuple(r["color"]) if r.get("color") else None
        if len(px) != 1 or (want and px != {want}):
            F.append("IMAGE_REGION: image %d region %s has %d distinct colors%s" % (x, r["rect"], len(px), "" if len(px) != 1 else " (%s != %s)" % (px, want)))
    if not hit:
        F.append("IMAGE_REGION: no image intersects %s (expected one)" % r["rect"])
    return F


def check_isolated(path, spec, timeout=120):
    try:
        p = subprocess.run([sys.executable, __file__, "--worker"], input=json.dumps({"path": str(path), "spec": spec}), capture_output=True, text=True, timeout=timeout)
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
