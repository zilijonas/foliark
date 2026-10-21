#!/usr/bin/env python3
"""Appearance / text-mapping / layout fidelity oracle for OUTPUT vs INPUT PDFs (NEW-21). Never imports foliark; PyMuPDF + pypdf only.

  .venv/bin/python fidelity.py in.pdf out.pdf --changed 0:x0,y0,x1,y1 [--changed ...] [--preserve TEXT] [--raster] [--spec spec.json]
  spec: {"changed":[{"page":0,"rect":[x0,y0,x1,y1]}], "preserve":["TEXT"], "raster":false, "dpi":72, "margin":2, "threshold":32,
         "max_frac":0.0002, "text_tol":1.0, "password":""}      rects are display space (spec section 2), points

Everything outside the declared changed regions (+margin) must look and read the same. Exit 0 iff no finding.
Importable: check_isolated(in, out, spec) -> (findings ["CODE: detail"], notes [str]).
Finding codes: OPEN PAGES SIZE PIXELS WORDS_FITZ WORDS_PYPDF PRESERVE_FITZ PRESERVE_PYPDF RASTER_TEXT TIMEOUT CRASH.
"""
import json
import re
import subprocess
import sys

import fitz
from pypdf import PdfReader

import objects
from objects import grow, unrot

DEFAULTS = {"dpi": 72, "margin": 2.0, "threshold": 32, "max_frac": 0.0002, "text_tol": 1.0}


def pixel_diff(pa, pb, masks, thr):
    """(max abs channel diff, count over thr, unmasked pixels) between two same-size RGB pixmaps, masked pixel rects excluded."""
    w, h, s = pa.width, pa.height, pa.stride
    a, b, m = bytearray(pa.samples), bytearray(pb.samples), bytearray(w * h)
    for x0, y0, x1, y1 in masks:
        x0, y0, x1, y1 = max(0, x0), max(0, y0), min(w, x1), min(h, y1)
        for y in range(y0, y1):
            m[y * w + x0:y * w + x1] = b"\1" * (x1 - x0)
            a[y * s + x0 * 3:y * s + x1 * 3] = b[y * s + x0 * 3:y * s + x1 * 3] = bytes(3 * (x1 - x0))
    total = m.count(0)
    if a == b:
        return 0, 0, total
    mx = over = 0
    for y in range(h):
        ra, rb = a[y * s:y * s + w * 3], b[y * s:y * s + w * 3]
        if ra == rb:
            continue
        for i in range(0, w * 3, 3):
            d = max(abs(ra[i] - rb[i]), abs(ra[i + 1] - rb[i + 1]), abs(ra[i + 2] - rb[i + 2]))
            mx, over = max(mx, d), over + (d > thr)
    return mx, over, total


def fitz_words(pg):
    """[(text, Rect)] in PyMuPDF's unrotated page space, plus line rects keyed by (block, line)."""
    ws, lines = [], {}
    for x0, y0, x1, y1, t, bl, ln, _ in pg.get_text("words"):
        r = fitz.Rect(x0, y0, x1, y1)
        ws.append((t, r))
        lines[(bl, ln)] = lines.get((bl, ln), fitz.Rect(r)) | r
    return ws, list(lines.values())


def pypdf_chunks(pg_fitz, pg_pypdf):
    """[(squashed text, x, y)] text-show start points (operator granularity) mapped into unrotated page space."""
    out = []

    def v(text, cm, tm, fd, fs):
        t = re.sub(r"\s+", "", text)
        if t:
            x, y = tm[4] * cm[0] + tm[5] * cm[2] + cm[4], tm[4] * cm[1] + tm[5] * cm[3] + cm[5]
            out.append((t, *tuple(fitz.Point(x, y) * pg_fitz.transformation_matrix)))
    pg_pypdf.extract_text(visitor_text=v)
    return out


def match_words(a, b, regions, tol):
    """Words of a outside regions that have no same-text, bbox-within-tol partner among b's words outside regions."""
    live = [(t, r) for t, r in b if not any(r.intersects(g) for g in regions)]
    miss = []
    for t, r in a:
        if any(r.intersects(g) for g in regions):
            continue
        k = next((i for i, (u, q) in enumerate(live) if u == t and max(abs(q.x0 - r.x0), abs(q.y0 - r.y0), abs(q.x1 - r.x1), abs(q.y1 - r.y1)) <= tol), None)
        if k is None:
            miss.append((t, tuple(round(v, 1) for v in r)))
        else:
            live.pop(k)
    return miss


def check(out_path, spec):
    spec = {**DEFAULTS, **spec}
    F, N = [], []
    try:
        da, db = fitz.open(spec["input"]), fitz.open(out_path)
        for d in (da, db):
            if d.needs_pass and not d.authenticate(spec.get("password", "")):
                return ["OPEN: password rejected"], N
    except Exception as e:
        return ["OPEN: %s" % e], N
    if len(da) != len(db):
        return ["PAGES: %d in, %d out" % (len(da), len(db))], N
    k, raster, tol = spec["dpi"] / 72, spec.get("raster"), spec["text_tol"]
    try:
        ra, rb = PdfReader(spec["input"], strict=False, password=spec.get("password") or None), PdfReader(out_path, strict=False, password=spec.get("password") or None)
    except Exception as e:
        return ["OPEN: pypdf: %s" % e], N
    for i, (pa, pb) in enumerate(zip(da, db)):
        regs = [grow(unrot(pa, c["rect"]), spec["margin"]) for c in spec.get("changed", []) if c["page"] == i]
        if abs(pa.rect.width - pb.rect.width) > .01 or abs(pa.rect.height - pb.rect.height) > .01:
            F.append("SIZE: page %d %s in, %s out" % (i, tuple(pa.rect), tuple(pb.rect)))
            continue
        # appearance: same-dpi renders, declared regions masked out
        masks = [tuple(round(v * k) for v in (c["rect"][0] - spec["margin"], c["rect"][1] - spec["margin"], c["rect"][2] + spec["margin"] + .5, c["rect"][3] + spec["margin"] + .5))
                 for c in spec.get("changed", []) if c["page"] == i]
        xa, xb = pa.get_pixmap(dpi=spec["dpi"], alpha=False), pb.get_pixmap(dpi=spec["dpi"], alpha=False)
        if (xa.width, xa.height) != (xb.width, xb.height):
            F.append("SIZE: page %d render %dx%d in, %dx%d out" % (i, xa.width, xa.height, xb.width, xb.height))
            continue
        mx, over, total = pixel_diff(xa, xb, masks, spec["threshold"])
        N.append("page %d: max diff %d, %d/%d unmasked px over %d (%.4f%%, limit %.4f%%)" % (i, mx, over, total, spec["threshold"], 100 * over / max(total, 1), 100 * spec["max_frac"]))
        if over > spec["max_frac"] * total:
            F.append("PIXELS: page %d: %d of %d unmasked pixels differ by > %d (max diff %d)" % (i, over, total, spec["threshold"], mx))
        wa, la = fitz_words(pa)
        wb, _ = fitz_words(pb)
        if raster:
            ct = re.sub(r"\s+", "", rb.pages[i].extract_text() or "")
            fonts = pb.get_fonts()
            if wb or ct or fonts:
                F.append("RASTER_TEXT: page %d declared raster but has text layer (%d words, %d pypdf chars, %d fonts)" % (i, len(wb), len(ct), len(fonts)))
            else:
                N.append("page %d: RASTER output has no text layer (%d input words lost, declared)" % (i, len(wa)))
            continue
        # text mapping, PyMuPDF: words outside regions, both directions
        for side, a, b in (("lost", wa, wb), ("appeared", wb, wa)):
            miss = match_words(a, b, regs, tol)
            if miss:
                F.append("WORDS_FITZ: page %d %d words %s outside changed regions (tol %.1f pt): %s" % (i, len(miss), side, tol, miss[:4]))
        # text mapping, pypdf: chunk start points of lines that do not touch a region
        ca, cb = pypdf_chunks(pa, ra.pages[i]), pypdf_chunks(pb, rb.pages[i])
        touched = [ln for ln in la if any(ln.intersects(g) for g in regs)]
        miss = []
        for t, x, y in ca:
            pt = fitz.Point(x, y)
            if any(grow(g, 1.5).contains(pt) for g in touched + regs):
                continue
            j = next((n for n, (u, p, q) in enumerate(cb) if u == t and abs(p - x) <= tol and abs(q - y) <= tol), None)
            if j is None:
                miss.append((t[:20], round(x, 1), round(y, 1)))
            else:
                cb.pop(j)
        if miss:
            F.append("WORDS_PYPDF: page %d %d text chunks missing or moved outside changed lines (tol %.1f pt): %s" % (i, len(miss), tol, miss[:4]))
    if not raster:
        sq = lambda x: re.sub(r"\s+", "", x)
        tf = sq("".join(p.get_text("text", flags=0) for p in db))
        try:
            tp = sq("".join(p.extract_text() or "" for p in rb.pages))
        except Exception as e:
            tp = ""
            F.append("PRESERVE_PYPDF: extractor failed: %s" % e)
        for s in spec.get("preserve", []):
            if sq(s) not in tf:
                F.append("PRESERVE_FITZ: %r not extractable" % s)
            if sq(s) not in tp:
                F.append("PRESERVE_PYPDF: %r not extractable" % s)
    return F, N


def check_isolated(inp, out, spec, timeout=180):
    r = objects.check_isolated(out, {**spec, "input": str(inp)}, timeout, script=__file__)
    return (r[0], r[1]) if r and isinstance(r[0], list) else (r, [])


def main(a):
    if a[:1] == ["--worker"]:
        j = json.load(sys.stdin)
        res = check(j["path"], j["spec"])
        sys.stdout.flush()
        print("\n@@" + json.dumps(res))
        return 0
    if len(a) < 2 or a[0].startswith("-"):
        print(__doc__)
        return 2
    spec, i = {"changed": [], "preserve": []}, 2
    while i < len(a):
        if a[i] == "--spec":
            spec.update(json.load(open(a[i + 1])))
        elif a[i] == "--changed":
            pg, rc = a[i + 1].split(":")
            spec["changed"].append({"page": int(pg), "rect": [float(v) for v in rc.split(",")]})
        elif a[i] == "--preserve":
            spec["preserve"].append(a[i + 1])
        elif a[i] == "--raster":
            spec["raster"] = True
            i -= 1
        else:
            print("unknown arg", a[i])
            return 2
        i += 2
    f, notes = check_isolated(a[0], a[1], spec)
    for x in notes:
        print("note", x)
    for x in f:
        print("FAIL", x)
    print("%s: %d findings" % (a[1], len(f)))
    return 1 if f else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
