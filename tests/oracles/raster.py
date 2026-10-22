#!/usr/bin/env python3
"""Independent raster-export probes (NEW-55, NEW-56): PyMuPDF only, knows nothing of foliark.

  raster.py geometry file.pdf [PASSWORD] -> per page "W H rotation images fonts" (display rect, points)
  raster.py redbox file.pdf PAGE SCALE   -> display-space bbox in points of the pure red pixels of a MuPDF render
  raster.py annots file.pdf [PASSWORD]   -> "annotations links widgets" over all pages (non-link non-widget with an appearance / links / widgets)
  raster.py image file.pdf PAGE out.rgb  -> decodes the page's one image, writes raw RGB, prints "W H components images"
"""
import sys
import fitz  # PyMuPDF


def geometry(path, password=""):
    doc = fitz.open(path)
    if doc.needs_pass:
        doc.authenticate(password)
    for p in doc:
        r = p.rect
        print(f"{r.width:.3f} {r.height:.3f} {p.rotation} {len(p.get_images(full=True))} {len(p.get_fonts(full=True))}")


def annots(path, password=""):
    doc = fitz.open(path)
    if doc.needs_pass:
        doc.authenticate(password)
    a = l = w = 0
    for p in doc:
        l += len(p.get_links())
        for an in p.annots() or []:
            a += an.type[0] not in (1, 16)  # links are counted above, popups carry no content
        w += len(list(p.widgets() or []))
    print(a, l, w)


def redbox(path, page, scale):
    p = fitz.open(path)[int(page)]
    s = float(scale)
    pm = p.get_pixmap(matrix=fitz.Matrix(s, s), alpha=False)
    d, w, h = pm.samples, pm.width, pm.height
    xs, ys = [], []
    for y in range(h):
        row = d[y * w * 3:(y + 1) * w * 3]
        for x in range(w):
            r, g, b = row[x * 3], row[x * 3 + 1], row[x * 3 + 2]
            if r > 200 and g < 60 and b < 60:
                xs.append(x)
                ys.append(y)
    if not xs:
        sys.exit("no red pixels")
    print(min(xs) / s, min(ys) / s, (max(xs) + 1) / s, (max(ys) + 1) / s)


def image(path, page, out):
    doc = fitz.open(path)
    imgs = doc[int(page)].get_images(full=True)
    if len(imgs) != 1:
        sys.exit(f"expected one image, found {len(imgs)}")
    pm = fitz.Pixmap(doc, imgs[0][0])
    if pm.alpha:
        sys.exit("unexpected alpha channel")
    if pm.colorspace.n != 3:
        pm = fitz.Pixmap(fitz.csRGB, pm)
    open(out, "wb").write(pm.samples)
    print(pm.width, pm.height, pm.n, len(imgs))


if __name__ == "__main__":
    {"geometry": geometry, "annots": annots, "redbox": redbox, "image": image}[sys.argv[1]](*sys.argv[2:])
