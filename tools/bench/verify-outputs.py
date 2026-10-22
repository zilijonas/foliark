# Dev-only independent oracle (PyMuPDF): what an exported PDF still contains. Usage: verify-outputs.py SPEC.json
# SPEC: [{"file": ..., "input": original PDF, "rects": [[x0, y0, x1, y1] page-0 display points]}]. Prints JSON.
# wordsInRegions must be 0 in the output; the same count on the input is the negative control (> 0).
import json, sys, fitz
def inside(page, rects):
    return sum(1 for w in page.get_text("words") for r in rects if (w[0] + w[2]) / 2 >= r[0] and (w[0] + w[2]) / 2 <= r[2] and (w[1] + w[3]) / 2 >= r[1] and (w[1] + w[3]) / 2 <= r[3])
out = []
for s in json.load(open(sys.argv[1])):
    d = fitz.open(s["file"])
    pages = [" ".join(p.get_text().split()) for p in d]
    text = " ".join(pages)
    imgs = sum(len(p.get_images()) for p in d)
    out.append({
        "file": s["file"].split("/")[-1], "pages": d.page_count, "bytes": len(open(s["file"], "rb").read()),
        "words": len(text.split()), "page0Words": len(pages[0].split()), "page1Words": len(pages[1].split()) if len(pages) > 1 else None,
        "images": imgs, "hasTextLayer": bool(text.strip()),
        "wordsInRegions": inside(d[0], s["rects"]), "wordsInRegionsOfInput": inside(fitz.open(s["input"])[0], s["rects"]), "inputPage0Words": len(fitz.open(s["input"])[0].get_text("words")),
        "metadataKeysSet": sorted(k for k, v in (d.metadata or {}).items() if v and k not in ("format", "encryption")),
        "embeddedFiles": d.embfile_count(),
    })
print(json.dumps(out))
