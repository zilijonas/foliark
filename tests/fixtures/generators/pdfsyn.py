"""Hand-written PDF syntax helpers (stdlib only, deterministic). Used by generate.py and tests/oracles/controls.py."""
import zlib

HDR = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n"
W, H = 300, 400


def stream(d, data, flate=False):
    if flate:
        data = zlib.compress(data, 9)
        d += " /Filter /FlateDecode"
    return ("<< %s /Length %d >>\nstream\n" % (d, len(data))).encode() + data + b"\nendstream"


def text(x, y, s, size=14, extra=""):
    return ("BT /F1 %d Tf %s%d %d Td (%s) Tj ET\n" % (size, extra, x, y, s)).encode()


def region(x, y, s, size=14):
    """Top-left display rect [x0,y0,x1,y1] over-covering Helvetica text drawn at PDF (x,y) on a W*H page."""
    return [x, round(H - y - 0.8 * size, 1), round(x + 0.55 * size * len(s), 1), round(H - y + 0.25 * size, 1)]


def build(objs, root=1, trailer="", base=None, prev=None):
    """Return (bytes, startxref). objs: {num: bytes|str}. With base+prev: append an incremental update."""
    out = bytearray(base if base else HDR)
    offs = {}
    for n in sorted(objs):
        b = objs[n]
        offs[n] = len(out)
        out += b"%d 0 obj\n" % n + (b.encode() if isinstance(b, str) else b) + b"\nendobj\n"
    xpos = len(out)
    size = max(offs) + 1
    tr = "/Root %d 0 R %s" % (root, trailer)
    if prev is None:
        lines = ["xref\n0 %d\n0000000000 65535 f \n" % size]
        for n in range(1, size):
            lines.append("%010d 00000 n \n" % offs[n] if n in offs else "0000000000 00000 f \n")
        out += "".join(lines).encode()
    else:
        out += b"xref\n"
        nums = sorted(offs)
        i = 0
        while i < len(nums):
            j = i
            while j + 1 < len(nums) and nums[j + 1] == nums[j] + 1:
                j += 1
            out += ("%d %d\n" % (nums[i], j - i + 1)).encode()
            out += "".join("%010d 00000 n \n" % offs[n] for n in nums[i:j + 1]).encode()
            i = j + 1
        tr += " /Prev %d" % prev
        size = max(size, 6)
    out += ("trailer\n<< /Size %d %s >>\nstartxref\n%d\n%%%%EOF\n" % (size, tr, xpos)).encode()
    return bytes(out), xpos


def simple(content, extra=None, page_extra="", cat_extra="", trailer="", flate=True, resources_extra="", fonts="/F1 5 0 R"):
    """1-page doc: 1 catalog, 2 pages, 3 page, 4 content, 5 Helvetica; extra objects from 6."""
    objs = {
        1: "<< /Type /Catalog /Pages 2 0 R %s >>" % cat_extra,
        2: "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %d %d] /Contents 4 0 R /Resources << /Font << %s >> %s >> %s >>"
        % (W, H, fonts, resources_extra, page_extra),
        4: stream("", content, flate),
        5: "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    }
    objs.update(extra or {})
    return build(objs, trailer=trailer)[0]
