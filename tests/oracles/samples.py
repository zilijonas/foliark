"""Image samples of an output PDF as JSON, read with pikepdf only (dev oracle for the redaction tests).
usage: samples.py out.pdf | samples.py --jpeg|--rgba file   (the latter: RGB or RGBA samples hex, decoded by PIL)
{"pages": [{"images": {"XName": image}, "content": latin1 text}, ...], "all": [image, ...]}  image = {num, w, h, cs, bpc, filter, mask, smask(num|null), data(hex of decoded samples)}
"""
import json
import sys

import pikepdf


def image(o):
    sm = o.get("/SMask")
    return {
        "num": o.objgen[0],
        "w": int(o.Width), "h": int(o.Height),
        "cs": str(o.get("/ColorSpace")), "bpc": int(o.get("/BitsPerComponent", 0)),
        "filter": str(o.get("/Filter")), "mask": bool(o.get("/ImageMask", False)),
        "smask": None if sm is None else sm.objgen[0],
        "decode": str(o.get("/Decode")), "has_mask": "/Mask" in o, "smask_in_data": int(o.get("/SMaskInData", 0)),
        "data": o.read_bytes().hex(),
    }


def is_image(o):
    return isinstance(o, pikepdf.Stream) and o.get("/Subtype") == "/Image"


if sys.argv[1] in ("--jpeg", "--rgba"):
    from PIL import Image
    print(Image.open(sys.argv[2]).convert("RGB" if sys.argv[1] == "--jpeg" else "RGBA").tobytes().hex())
    sys.exit(0)
pdf = pikepdf.open(sys.argv[1])
pages = []
for pg in pdf.pages:
    xo = pg.obj.get("/Resources", {}).get("/XObject", {})
    pages.append({"images": {str(k)[1:]: image(v) for k, v in xo.items() if is_image(v)},
                  "content": pg.obj.get("/Contents") is not None and "\n".join(c.read_bytes().decode("latin1") for c in (pg.obj.Contents if isinstance(pg.obj.Contents, pikepdf.Array) else [pg.obj.Contents])) or ""})
print(json.dumps({"pages": pages, "all": [image(o) for o in pdf.objects if is_image(o)]}))
