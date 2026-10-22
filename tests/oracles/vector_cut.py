#!/usr/bin/env python3
"""Independent geometry oracle for precise vector cuts (NEW-86, ADR 0009). Never imports foliark.

  .venv/bin/python vector_cut.py out.pdf --spec spec.json     # {"regions": [[x0,y0,x1,y1], ...], "tol": 0.005}

Regions are display space (spec section 2). Reads the page content (and Forms it draws) with pikepdf, tracks the CTM,
and checks, with no clip trusted to hide anything:
  VERTEX   a painted fill polygon or a clip outline has a vertex or an edge inside a region grown by -tol
  CONTAINED a region lies wholly inside a painted fill (no edge reaches it, the fill rule says inked)
  UNCLIPPED rendering the page with every W / W* removed still shows ink inside a region
  STROKE   (NEW-87) a solid stroke's centreline passes within its own half width (smallest axis of the CTM) of a region:
           a lower bound of its ink, so never a false alarm; miters, caps and dashes are left to UNCLIPPED
Curves are sampled at 32 steps. Exit 0 iff no finding. Importable: check(path, spec) -> [findings "CODE: detail"].
"""
import io
import json
import sys

import fitz
import pikepdf

from objects import unrot

FILL = {"f", "F", "f*", "B", "B*", "b", "b*"}
STROKE = {"S", "s", "B", "B*", "b", "b*"}
NUMERIC = {"m", "l", "c", "v", "y", "re", "cm"}


def mul(a, b):
    """Matrix product: apply a, then b (PDF row-vector convention)."""
    return (a[0] * b[0] + a[1] * b[2], a[0] * b[1] + a[1] * b[3], a[2] * b[0] + a[3] * b[2], a[2] * b[1] + a[3] * b[3],
            a[4] * b[0] + a[5] * b[2] + b[4], a[4] * b[1] + a[5] * b[3] + b[5])


def app(m, x, y):
    return (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])


def bez(p0, p1, p2, p3):
    out = []
    for i in range(1, 33):
        t = i / 32
        u = 1 - t
        out.append((u ** 3 * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t ** 3 * p3[0],
                    u ** 3 * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t ** 3 * p3[1]))
    return out


def smallest_axis(m):
    p, q, r = m[0] ** 2 + m[1] ** 2, m[2] ** 2 + m[3] ** 2, m[0] * m[2] + m[1] * m[3]
    return max((p + q) / 2 - (((p - q) / 2) ** 2 + r * r) ** .5, 0) ** .5


def walk(doc, stream_obj, resources, ctm, found, depth=0):
    """Collect (kind, rule, polygons in user space) for every painted fill and every clip outline; a stroke is
    ("stroke", (half width, dashed), polylines)."""
    if depth > 8:
        return
    q, subs, cur, start, clip = [], [], None, None, None
    lw, dashed = 1.0, False
    for ins in pikepdf.parse_content_stream(stream_obj):
        op = str(ins.operator)
        a = [float(x) for x in ins.operands] if op in NUMERIC else list(ins.operands)
        if op == "q":
            q.append((ctm, lw, dashed))
        elif op == "Q":
            ctm, lw, dashed = q.pop() if q else (ctm, lw, dashed)
        elif op == "w":
            lw = float(a[0])
        elif op == "d":
            dashed = sum(float(x) for x in a[0]) > 0
        elif op == "cm":
            ctm = mul(tuple(a), ctm)
        elif op == "m":
            cur = start = app(ctm, a[0], a[1])
            subs.append([cur])
        elif op == "l" and subs:
            cur = app(ctm, a[0], a[1])
            subs[-1].append(cur)
        elif op in ("c", "v", "y") and subs:
            pts = [app(ctm, a[i], a[i + 1]) for i in range(0, len(a), 2)]
            p1, p2, p3 = {"c": (pts[0], pts[1], pts[2]), "v": (cur, pts[0], pts[1]), "y": (pts[0], pts[1], pts[1])}[op]
            subs[-1].extend(bez(cur, p1, p2, p3))
            cur = subs[-1][-1]
        elif op == "re":
            x, y, w, h = a
            subs.append([app(ctm, x, y), app(ctm, x + w, y), app(ctm, x + w, y + h), app(ctm, x, y + h), app(ctm, x, y)])
            cur = subs[-1][0]
        elif op == "h" and subs:
            cur = subs[-1][0]
            subs[-1].append(cur)
        elif op in ("W", "W*"):
            clip = "even" if op == "W*" else "nonzero"
        elif op in FILL or op in STROKE or op == "n":
            if op in ("s", "b", "b*") and subs:
                subs[-1].append(subs[-1][0])
            if op in STROKE:
                found.append(("stroke", (lw * smallest_axis(ctm) / 2, dashed), [list(s) for s in subs]))
            if clip:
                found.append(("clip", clip, [list(s) for s in subs]))
            if op in FILL:
                found.append(("fill", "even" if op.endswith("*") else "nonzero", [list(s) for s in subs]))
            subs, cur, clip = [], None, None
        elif op == "Do":
            xo = (resources or {}).get("/XObject", {}).get(str(ins.operands[0])) if resources is not None else None
            if xo is not None and xo.get("/Subtype") == "/Form":
                fm = tuple(float(v) for v in xo.get("/Matrix", [1, 0, 0, 1, 0, 0]))
                walk(doc, xo, xo.get("/Resources", resources), mul(fm, ctm), found, depth + 1)


def seg_hits(a, b, r):
    t0, t1, dx, dy = 0.0, 1.0, b[0] - a[0], b[1] - a[1]
    for p, q in ((-dx, a[0] - r[0]), (dx, r[2] - a[0]), (-dy, a[1] - r[1]), (dy, r[3] - a[1])):
        if p == 0:
            if q < 0:
                return False
        else:
            t = q / p
            if p < 0:
                t0 = max(t0, t)
            else:
                t1 = min(t1, t)
            if t0 > t1:
                return False
    return True


def seg_rect_dist(a, b, r):
    """Distance from segment a-b to the box r (0 when they meet)."""
    if seg_hits(a, b, r):
        return 0.0

    def to_box(p):
        return ((max(r[0] - p[0], 0, p[0] - r[2])) ** 2 + (max(r[1] - p[1], 0, p[1] - r[3])) ** 2) ** .5

    def to_seg(p):
        dx, dy = b[0] - a[0], b[1] - a[1]
        t = 0.0 if dx == dy == 0 else max(0.0, min(1.0, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy)))
        return ((p[0] - a[0] - t * dx) ** 2 + (p[1] - a[1] - t * dy) ** 2) ** .5

    return min([to_box(a), to_box(b)] + [to_seg(c) for c in ((r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3]))])


def winding(polys, p):
    w = 0
    for c in polys:
        for i, a in enumerate(c):
            b = c[(i + 1) % len(c)]
            side = (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1])
            if a[1] <= p[1] < b[1] and side > 0:
                w += 1
            elif b[1] <= p[1] < a[1] and side < 0:
                w -= 1
    return w


def strip_clips(path):
    """The page with every W / W* removed (from the page and each Form it can reach), as PDF bytes."""
    pdf = pikepdf.open(path)
    seen = set()

    def fix(stream):
        if stream.objgen in seen:
            return
        seen.add(stream.objgen)
        ins = [i for i in pikepdf.parse_content_stream(stream) if str(i.operator) not in ("W", "W*")]
        stream.write(pikepdf.unparse_content_stream(ins))

    def forms(res):
        for _, xo in (res or {}).get("/XObject", {}).items():
            if xo.get("/Subtype") == "/Form":
                fix(xo)
                forms(xo.get("/Resources"))

    for page in pdf.pages:
        contents = page.obj.get("/Contents")
        data = b"\n".join(s.read_bytes() for s in (contents if isinstance(contents, pikepdf.Array) else [contents]))
        s = pdf.make_stream(data)
        page.obj["/Contents"] = s
        fix(s)
        forms(page.obj.get("/Resources"))
    out = io.BytesIO()
    pdf.save(out)
    return out.getvalue()


def check(path, spec):
    tol = spec.get("tol", 0.005)
    F = []
    doc = fitz.open(path)
    pdf = pikepdf.open(path)
    for pno, pg in enumerate(doc):
        regs = [unrot(pg, r) for r in spec["regions"]]
        core = [(r.x0 + tol, r.y0 + tol, r.x1 - tol, r.y1 - tol) for r in regs]
        page = pdf.pages[pno]
        found = []
        contents = page.obj.get("/Contents")
        data = b"\n".join(s.read_bytes() for s in (contents if isinstance(contents, pikepdf.Array) else [contents]))
        walk(pdf, pdf.make_stream(data), page.obj.get("/Resources"), (1, 0, 0, 1, 0, 0), found)
        tm = pg.transformation_matrix
        for kind, rule, subs in found:
            polys = [[tuple(fitz.Point(x, y) * tm) for x, y in s] for s in subs]
            if kind == "stroke":
                hw, dashed = rule
                for ri, r in enumerate(core):
                    if not dashed and any(seg_rect_dist(c[i], c[i + 1], r) < hw - tol for c in polys for i in range(len(c) - 1)):
                        F.append("STROKE: page %d stroke centreline is within its half width of region %d" % (pno, ri))
                continue
            for ri, r in enumerate(core):
                hit = any(r[0] < p[0] < r[2] and r[1] < p[1] < r[3] for c in polys for p in c) or any(
                    seg_hits(a, c[(i + 1) % len(c)], r) for c in polys for i, a in enumerate(c) if len(c) > 1)
                if hit:
                    F.append("VERTEX: page %d %s outline reaches region %d" % (pno, kind, ri))
                elif kind == "fill" and r[0] < r[2] and r[1] < r[3]:
                    w = winding(polys, ((r[0] + r[2]) / 2, (r[1] + r[3]) / 2))
                    if (w % 2 != 0) if rule == "even" else (w != 0):
                        F.append("CONTAINED: page %d region %d lies inside a painted fill" % (pno, ri))
    stripped = fitz.open(stream=strip_clips(path), filetype="pdf")
    for pno, pg in enumerate(stripped):
        for ri, r in enumerate(spec["regions"]):
            rc = fitz.Rect(r[0] + .6, r[1] + .6, r[2] - .6, r[3] - .6)  # a pixmap clip is in display (rotated) space
            pix = pg.get_pixmap(matrix=fitz.Matrix(4, 4), clip=rc, alpha=False)
            s, c = pix.samples, pix.n
            n = sum(1 for k in range(0, len(s), c) if min(s[k:k + c]) < 250)
            if n:
                F.append("UNCLIPPED: page %d region %d shows %d inked samples with clips removed" % (pno, ri, n))
    return F


def main(a):
    if len(a) < 3 or a[1] != "--spec":
        print(__doc__)
        return 2
    F = check(a[0], json.load(open(a[2])))
    for x in F:
        print("FAIL", x)
    print("%s: %d findings" % (a[0], len(F)))
    return 1 if F else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
