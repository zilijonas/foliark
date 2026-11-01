#!/usr/bin/env python3
"""Push-down shift oracle (NEW-130, ADR 0015). Dev-only; reads both PDFs with PyMuPDF and knows nothing about foliark-*.

  shift.py in.pdf out.pdf --region PAGE:x0,y0,x1,y1 --dy DY [--changed PAGE:x0,y0,x1,y1 ...] [--tol 0.05]

Display space (top-left origin, y down, as fidelity.py); PAGE is 0-based; DY > 0 moves content down. R is the region a push-down
moves: every word, vector drawing (page.get_drawings), image and link annotation of the INPUT lying fully inside R must exist in the
OUTPUT at the same place plus DY (x unchanged), with the same text/paint/target. Everything else of the input (outside R and outside
the --changed rects, where an edit may rewrite text) must be where it was. Output items that neither of those explain, and an R that
cuts an item (partly inside), are findings. `check(in, out, page, region, dy, changed=(), tol=0.05)` returns a list of `CODE: detail`
strings, empty means pass. Exit 0 pass, 1 findings, 2 usage/open error.
"""
import argparse
import sys

import fitz

MOVED, FIXED, EXTRA, STRADDLE, PAGES = "SHIFT_MOVED", "SHIFT_FIXED", "SHIFT_EXTRA", "SHIFT_STRADDLE", "SHIFT_PAGES"


def items(page):
    """(kind, key, display rect) for every word, drawing, image and link of the page."""
    m = page.rotation_matrix
    box = lambda r: tuple(fitz.Rect(r).transform(m))  # noqa: E731
    out = [("word", w[4], box(w[:4])) for w in page.get_text("words")]
    out += [("draw", "%s %s %s %d" % (d["type"], d.get("color"), d.get("fill"), len(d["items"])), box(d["rect"])) for d in page.get_drawings()]
    out += [("image", "%dx%d" % (i["width"], i["height"]), box(i["bbox"])) for i in page.get_image_info()]
    out += [("link", "%s %s %s" % (k["kind"], k.get("uri"), k.get("page")), box(k["from"])) for k in page.get_links()]
    return out


def touches(r, q):  # closed boxes: an item on the edge of a region counts as touching it
    return r[0] <= q[2] and r[2] >= q[0] and r[1] <= q[3] and r[3] >= q[1]


def inside(r, q, eps=1e-3):
    return r[0] >= q[0] - eps and r[2] <= q[2] + eps and r[1] >= q[1] - eps and r[3] <= q[3] + eps


def shifted(r, dy):
    return (r[0], r[1] + dy, r[2], r[3] + dy)


def take(pool, kind, key, r, tol):
    """Remove and return the first pool item equal to (kind, key) with every edge within tol of r, else None."""
    for j, (k, ky, q) in enumerate(pool):
        if (k, ky) == (kind, key) and all(abs(a - b) <= tol for a, b in zip(r, q)):
            return pool.pop(j)
    return None


def fmt(kind, key, r):
    return "%s %r at [%.2f,%.2f,%.2f,%.2f]" % (kind, key if kind != "draw" else key.split()[0], *r)


def check(src, dst, page, region, dy, changed=(), tol=0.05):
    a, b = fitz.open(src), fitz.open(dst)
    if len(a) != len(b):
        return ["%s: %d pages in, %d out" % (PAGES, len(a), len(b))]
    pa, pb = a[page], b[page]
    if pa.rect != pb.rect:
        return ["%s: page %d size %s in, %s out" % (PAGES, page, pa.rect, pb.rect)]
    findings, pool, want = [], items(pb), []
    for kind, key, r in items(pa):
        if touches(r, region):
            if inside(r, region):
                want.append((MOVED, kind, key, shifted(r, dy)))
            else:
                findings.append("%s: region cuts %s" % (STRADDLE, fmt(kind, key, r)))
        elif not any(touches(r, c) for c in changed):
            want.append((FIXED, kind, key, r))
    for code, kind, key, r in want:
        if take(pool, kind, key, r, tol) is None:
            findings.append("%s: %s not found%s" % (code, fmt(kind, key, r), " (expected pushed %+.2f pt)" % dy if code == MOVED else ""))
    for kind, key, r in pool:
        if not any(touches(r, c) for c in changed):
            findings.append("%s: output has %s that no input item explains" % (EXTRA, fmt(kind, key, r)))
    return findings


def rect_arg(s):
    page, _, box = s.partition(":")
    box = [float(v) for v in box.split(",")]
    if len(box) != 4:
        raise argparse.ArgumentTypeError("want PAGE:x0,y0,x1,y1")
    return int(page), box


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("input")
    ap.add_argument("output")
    ap.add_argument("--region", type=rect_arg, required=True, help="PAGE:x0,y0,x1,y1 the push moves (display space)")
    ap.add_argument("--dy", type=float, required=True, help="points the region moves down")
    ap.add_argument("--changed", type=rect_arg, action="append", default=[], help="PAGE:x0,y0,x1,y1 where the edit may rewrite text")
    ap.add_argument("--tol", type=float, default=0.05)
    n = ap.parse_args()
    page, region = n.region
    try:
        findings = check(n.input, n.output, page, region, n.dy, [c for p, c in n.changed if p == page], n.tol)
    except Exception as e:  # unreadable file or page out of range: fail loudly, never pass
        print("OPEN: %s" % e, file=sys.stderr)
        return 2
    print(*findings or ["shift ok: region %s moved %+.2f pt" % (region, n.dy)], sep="\n")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
