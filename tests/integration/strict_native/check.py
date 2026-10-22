#!/usr/bin/env python3
"""Independent checks of strict native exports, run once by crates/foliark-engine/tests/strict_native.rs.
Usage: check.py CASES.json  (stdout: one JSON object {case id: {check: [findings]}}; every list must be empty).
Per case the Rust side hands over: input path (+password), the seeded secrets, the public words that must stay, the redacted
regions (display space, points; kind sensitive | sensitive-image | sensitive-vector | region), flags, and the two outputs (all
redactions placed / sanitize-only). Nothing here imports foliark; it uses tests/oracles (pikepdf, pypdf, PyMuPDF).
 input      each secret is findable in the INPUT by objects.py (decoded, any depth): the control proves the checks can see it
 redacted   objects.py (forbidden secrets, image regions black, vector regions only our fill) + fidelity.py (everything outside
            the redactions and the secrets' own words looks and reads the same) + PyMuPDF: public words present
 sanitize   objects.py (secrets PyMuPDF cannot read from the input page must be gone) + fidelity.py with no declared change
"""
import json
import os
import re
import pathlib
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "oracles"))
import fitz  # noqa: E402
import fidelity  # noqa: E402
import objects  # noqa: E402
import pikepdf  # noqa: E402


def plain_copy(path, pw, tmp):
    if pw is None:
        return path
    out = os.path.join(tmp, "plain.pdf")
    pikepdf.open(path, password=pw).save(out, encryption=False)
    return out


def page_text(doc):
    return "".join(p.get_text() for p in doc)


def secret_rects(doc, secrets):
    """Display-space rects of every PyMuPDF hit of a secret in the input (search, and the words that contain it: search skips
    invisible text, words do not). PyMuPDF space -> display via the page rotation."""
    out = []
    for i, pg in enumerate(doc):
        words = pg.get_text("words")
        for s in secrets:
            rects = pg.search_for(s) + [fitz.Rect(w[:4]) for w in words if s in w[4]]
            out += [{"page": i, "rect": list((r * pg.rotation_matrix).normalize())} for r in rects]
    return out


def no_ap_rects(doc):
    out = []
    for i, pg in enumerate(doc):
        for a in pg.annots() or []:
            if doc.xref_get_key(a.xref, "AP")[0] == "null":
                out.append({"page": i, "rect": list((a.rect * pg.rotation_matrix).normalize())})
    return out


def judge(out_path, inp, case, spec_obj, changed, label):
    r = {"objects": objects.check_isolated(out_path, spec_obj)}
    fid, _ = fidelity.check_isolated(inp, out_path, {"changed": changed, "preserve": case["keep"]})
    if case["flags"].get("layers"):  # pypdf never evaluates optional content: its chunk check cannot judge removed layers
        fid = [f for f in fid if not f.startswith("WORDS_PYPDF")]
    if case["flags"].get("actualtext"):  # ActualText is dropped by design (spec 8.2): the extractors read the drawn glyphs instead
        fid = [f for f in fid if not f.startswith("WORDS_")]
    # pypdf places text shown inside a transformed Form or on a rotated page wrongly (it has no matrix for them); a chunk it
    # "lost" that is a redacted secret is the redaction working. Any other lost chunk stays a finding (PyMuPDF judges words too).
    fid = [f for f in fid if not (f.startswith("WORDS_PYPDF") and set(re.findall(r"\('([^']*)', ", f)) <= set(case["secrets"]) and "text chunks missing" in f and ("redacted" in label))]
    r["fidelity"] = fid
    return r


def main(path):
    res = {}
    for case in json.load(open(path)):
        tmp = tempfile.mkdtemp()
        inp = plain_copy(case["input"], case["password"], tmp)
        doc = fitz.open(inp)
        secrets, regions = case["secrets"], case["regions"]
        visible = page_text(doc)
        flat = lambda s: "".join(s.split())
        r = {}
        # the control: objects.py must see every secret in the input (else a clean output proves nothing)
        found = objects.check_isolated(inp, {"forbidden": secrets, "allow": ["metadata", "embeddedfiles", "javascript", "openaction", "aa", "acroform", "ocproperties", "info"]})
        r["input_control"] = ["secret %r not found in the input by objects.py" % s for s in secrets if not any(repr(s) in f for f in found)]
        # redacted output
        spec = {"forbidden": secrets,
                "image_regions": [{"page": g["page"], "rect": g["rect"], "color": [0, 0, 0]} for g in regions if g["kind"] == "sensitive-image"],
                "vector_regions": [{"page": g["page"], "rect": g["rect"], "fill": [0, 0, 0]} for g in regions if g["kind"] == "sensitive-vector"]}
        # Image erasure is a closed-set test (a sample whose square only touches the region edge is erased too): the declared change of an
        # image region grows by one source sample (4 pt here); objects.py still demands every sample inside the region itself is black.
        grow = lambda g: [g["rect"][0] - 4, g["rect"][1] - 4, g["rect"][2] + 4, g["rect"][3] + 4] if g["kind"] == "sensitive-image" else g["rect"]
        changed = [{"page": g["page"], "rect": grow(g)} for g in regions] + secret_rects(doc, secrets) + no_ap_rects(doc)
        r["redacted"] = judge(case["redacted"], inp, case, {**spec, "password": ""}, changed, "redacted")
        # sanitize-only: only what the input page itself shows may remain; the rest must be gone
        gone = [s for s in secrets if flat(s) not in flat(visible)]
        r["sanitize"] = judge(case["sanitize"], inp, case, {"forbidden": gone}, no_ap_rects(doc), "sanitize")
        r["sanitize_gone"] = gone
        res[case["id"]] = r
    print("\n@@" + json.dumps(res))


if __name__ == "__main__":
    main(sys.argv[1])
