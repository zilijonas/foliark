#!/usr/bin/env python3
"""Deterministic synthetic scanned-page PDFs (NEW-81), one image format per file, A4 at 300 dpi (2480x3508).

  scans.py OUTDIR [--pages N]   writes scan-<kind>.pdf for every kind below, prints "kind bytes sha256"

Not in tests/fixtures/manifest.json: each file is 0.4 to 3.5 MB per page, over the corpus' 200 KB cap, and is
regenerated on demand (tools/bench). Pure Python + Pillow (libjpeg/libtiff) + our own JBIG2 MQ encoder, so no
C PDF engine is involved. Page content is invented: ruled boxes and rows of stroke "glyphs" on noisy paper.
"""
import hashlib
import io
import random
import sys
import zlib
from pathlib import Path

from PIL import Image, ImageDraw

sys.path.insert(0, str(Path(__file__).resolve().parent))
from pdfsyn import build, stream  # noqa: E402

W, H = 2480, 3508
KINDS = ("jpeg-baseline-rgb", "jpeg-baseline-gray", "jpeg-progressive-rgb", "jpeg-progressive-gray", "jbig2", "ccitt-g4", "flate-gray")


def page_gray(seed):
    rng = random.Random(seed)
    paper = Image.frombytes("L", (W, H), rng.randbytes(W * H)).point(lambda v: 240 + (v >> 6))
    d = ImageDraw.Draw(paper)
    d.rectangle((250, 250, W - 250, 420), outline=40, width=4)
    for y in range(480, H - 300, 62):
        x = 250
        while x < W - 300:
            for _ in range(rng.randint(2, 9)):
                w = rng.randint(10, 26)
                ink = rng.randint(25, 70)
                for _ in range(rng.randint(2, 3)):
                    d.line((x + rng.randint(0, w), y + rng.randint(0, 34), x + rng.randint(0, w), y + rng.randint(0, 34)), fill=ink, width=rng.randint(3, 4))
                x += w + rng.randint(4, 8)
            x += 24
        if rng.random() < 0.12:
            d.line((250, y + 48, W - 250, y + 48), fill=60, width=3)
    for _ in range(2500):
        x, y = rng.randrange(W - 4), rng.randrange(H - 4)
        d.rectangle((x, y, x + rng.randint(0, 3), y + rng.randint(0, 3)), fill=rng.randint(120, 200))
    return paper


def page_rgb(seed):
    g = page_gray(seed)
    return Image.merge("RGB", (g, g.point(lambda v: v - 3), g.point(lambda v: v - 10)))


def jpeg(im, progressive):
    b = io.BytesIO()
    im.save(b, "JPEG", quality=75, progressive=progressive, optimize=progressive)
    return b.getvalue()


def ccitt_g4(bw):
    b = io.BytesIO()
    bw.save(b, "TIFF", compression="group4", tiffinfo={278: H})
    t = Image.open(io.BytesIO(b.getvalue()))
    off, cnt = t.tag_v2[273], t.tag_v2[279]
    assert len(off) == 1, "need a single strip"
    # PIL's strip comes out inverted w.r.t. the default (checked against MuPDF), hence /BlackIs1 true
    return b.getvalue()[off[0]:off[0] + cnt[0]]


# --- JBIG2: generic region, arithmetic coding, template 0, typical prediction (T.88 6.2, Annex E) ---
QE = [(0x5601, 1, 1, 1), (0x3401, 2, 6, 0), (0x1801, 3, 9, 0), (0x0AC1, 4, 12, 0), (0x0521, 5, 29, 0), (0x0221, 38, 33, 0), (0x5601, 7, 6, 1), (0x5401, 8, 14, 0), (0x4801, 9, 14, 0), (0x3801, 10, 14, 0), (0x3001, 11, 17, 0), (0x2401, 12, 18, 0), (0x1C01, 13, 20, 0), (0x1601, 29, 21, 0), (0x5601, 15, 14, 1), (0x5401, 16, 14, 0), (0x5101, 17, 15, 0), (0x4801, 18, 16, 0), (0x3801, 19, 17, 0), (0x3401, 20, 18, 0), (0x3001, 21, 19, 0), (0x2801, 22, 19, 0), (0x2401, 23, 20, 0), (0x2201, 24, 21, 0), (0x1C01, 25, 22, 0), (0x1801, 26, 23, 0), (0x1601, 27, 24, 0), (0x1401, 28, 25, 0), (0x1201, 29, 26, 0), (0x1101, 30, 27, 0), (0x0AC1, 31, 28, 0), (0x09C1, 32, 29, 0), (0x08A1, 33, 30, 0), (0x0521, 34, 31, 0), (0x0441, 35, 32, 0), (0x02A1, 36, 33, 0), (0x0221, 37, 34, 0), (0x0141, 38, 35, 0), (0x0111, 39, 36, 0), (0x0085, 40, 37, 0), (0x0049, 41, 38, 0), (0x0025, 42, 39, 0), (0x0015, 43, 40, 0), (0x0009, 44, 41, 0), (0x0005, 45, 42, 0), (0x0001, 45, 43, 0), (0x5601, 46, 46, 0)]


class MQ:
    def __init__(self):
        self.a, self.c, self.ct, self.out = 0x8000, 0, 12, bytearray(1)  # out[0] is the discarded BPST-1 byte
        self.i, self.mps = [0] * 65536, [0] * 65536

    def _byteout(self):
        o = self.out
        if o[-1] == 0xFF:
            o.append((self.c >> 20) & 0xFF); self.c &= 0xFFFFF; self.ct = 7
        elif self.c < 0x8000000:
            o.append((self.c >> 19) & 0xFF); self.c &= 0x7FFFF; self.ct = 8
        else:
            o[-1] += 1
            if o[-1] == 0xFF:
                self.c &= 0x7FFFFFF
                o.append((self.c >> 20) & 0xFF); self.c &= 0xFFFFF; self.ct = 7
            else:
                o.append((self.c >> 19) & 0xFF); self.c &= 0x7FFFF; self.ct = 8

    def encode(self, cx, bit):
        qe, nmps, nlps, sw = QE[self.i[cx]]
        a = self.a - qe
        if bit == self.mps[cx]:
            if a & 0x8000:
                self.a, self.c = a, self.c + qe
                return
            if a < qe:
                a = qe
            else:
                self.c += qe
            self.i[cx] = nmps
        else:
            if a < qe:
                self.c += qe
            else:
                a = qe
            if sw:
                self.mps[cx] ^= 1
            self.i[cx] = nlps
        while True:
            a <<= 1; self.c <<= 1; self.ct -= 1
            if self.ct == 0:
                self._byteout()
            if a & 0x8000:
                break
        self.a = a

    def flush(self):
        t = self.c + self.a
        self.c |= 0xFFFF
        if self.c >= t:
            self.c -= 0x8000
        self.c <<= self.ct; self._byteout()
        self.c <<= self.ct; self._byteout()
        if self.out[-1] != 0xFF:
            self.out.append(0xFF)
        self.out.append(0xAC)
        return bytes(self.out[1:])


def jbig2_generic(bw):
    """bw: PIL mode '1' (0 = black). Returns an embedded JBIG2 stream (page info + one immediate generic region)."""
    raw = bw.convert("L").point(lambda v: 0 if v else 1).tobytes()  # one byte per pixel, 1 = black
    z = bytes(W + 8)
    rows = [z, z] + [b"\0\0\0\0" + raw[y * W:(y + 1) * W] + b"\0\0\0\0" for y in range(H)]
    mq, ltp = MQ(), 0
    for y in range(H):
        r, r1, r2 = rows[y + 2], rows[y + 1], rows[y]
        same = r == r1
        mq.encode(0x9B25, int(same) ^ ltp)
        ltp = int(same)
        if same:
            continue
        for x in range(4, W + 4):
            # jbig2dec template 0 labelling, AT = (3,-1) (-3,-1) (2,-2) (-2,-2)
            cx = (r[x - 1] | r[x - 2] << 1 | r[x - 3] << 2 | r[x - 4] << 3 | r1[x + 3] << 4 | r1[x + 2] << 5 | r1[x + 1] << 6 | r1[x] << 7
                  | r1[x - 1] << 8 | r1[x - 2] << 9 | r1[x - 3] << 10 | r2[x + 2] << 11 | r2[x + 1] << 12 | r2[x] << 13 | r2[x - 1] << 14 | r2[x - 2] << 15)
            mq.encode(cx, r[x])
    data = mq.flush()
    seg = lambda n, typ, body: n.to_bytes(4, "big") + bytes([typ, 0, 1]) + len(body).to_bytes(4, "big") + body
    info = W.to_bytes(4, "big") + H.to_bytes(4, "big") + (11811).to_bytes(4, "big") * 2 + b"\0\0\0"
    region = W.to_bytes(4, "big") + H.to_bytes(4, "big") + bytes(8) + b"\0" + bytes([0x08]) + bytes([3, 0xFF, 0xFD, 0xFF, 2, 0xFE, 0xFE, 0xFE]) + data
    return seg(0, 48, info) + seg(1, 38, region)


def pdf(kind, n):
    objs, kids = {}, []
    base = 3
    for p in range(n):
        seed = 7000 + p
        page, content, img = base + 3 * p, base + 3 * p + 1, base + 3 * p + 2
        common = "/Type /XObject /Subtype /Image /Width %d /Height %d" % (W, H)
        if kind.startswith("jpeg"):
            gray = kind.endswith("gray")
            im = page_gray(seed) if gray else page_rgb(seed)
            data = jpeg(im, "progressive" in kind)
            ximg = stream("%s /ColorSpace /%s /BitsPerComponent 8 /Filter /DCTDecode" % (common, "DeviceGray" if gray else "DeviceRGB"), data)
        elif kind == "flate-gray":
            ximg = stream("%s /ColorSpace /DeviceGray /BitsPerComponent 8" % common, page_gray(seed).tobytes(), flate=True)
        else:
            bw = page_gray(seed).point(lambda v: 255 if v > 128 else 0).convert("1")
            if kind == "ccitt-g4":
                ximg = stream("%s /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns %d /Rows %d /BlackIs1 true >>" % (common, W, H), ccitt_g4(bw))
            else:
                ximg = stream("%s /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /JBIG2Decode" % common, jbig2_generic(bw))
        objs[img] = ximg
        objs[content] = stream("", b"q 595.276 0 0 841.89 0 0 cm /Im0 Do Q\n", flate=True)
        objs[page] = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595.276 841.89] /Contents %d 0 R /Resources << /XObject << /Im0 %d 0 R >> >> >>" % (content, img)
        kids.append("%d 0 R" % page)
    objs[1] = "<< /Type /Catalog /Pages 2 0 R >>"
    objs[2] = "<< /Type /Pages /Kids [%s] /Count %d >>" % (" ".join(kids), n)
    return build(objs)[0]


if __name__ == "__main__":
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    pages = int(sys.argv[sys.argv.index("--pages") + 1]) if "--pages" in sys.argv else 3
    for kind in KINDS:
        data = pdf(kind, pages)
        (out / ("scan-%s.pdf" % kind)).write_bytes(data)
        print(kind, len(data), hashlib.sha256(data).hexdigest())
