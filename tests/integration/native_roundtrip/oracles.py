#!/usr/bin/env python3
"""Independent checks of one native round trip, run by crates/foliark-engine/tests/integration_roundtrip.rs.
Usage: oracles.py IN.pdf OUT.pdf [--password PW] [--pending]
Prints one JSON object: {"objects": [...], "fidelity": [...], "words_fitz": [...], "words_pypdf": [...]}; every list holds
findings, so an empty list passes. Encrypted inputs are compared through a pikepdf-decrypted copy. With --pending
(annotations/widgets are dropped until flattening exists) fidelity is expected to see the loss and is reported, not judged.
Words: PyMuPDF words and pypdf text of the OUTPUT must equal those of the INPUT (text, order, boxes within 0.5 pt)."""
import json
import os
import pathlib
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "oracles"))
import fitz  # noqa: E402
import fidelity  # noqa: E402
import objects  # noqa: E402
import pikepdf  # noqa: E402
import pypdf  # noqa: E402

TOL = 0.5


def fitz_words(path):
    with fitz.open(path) as d:
        return [[(w[4], w[:4]) for w in p.get_text("words", sort=True)] for p in d]


def words_findings(a, b):
    out = []
    if len(a) != len(b):
        return [f"page count {len(a)} != {len(b)}"]
    for i, (x, y) in enumerate(zip(a, b)):
        if [w[0] for w in x] != [w[0] for w in y]:
            out.append(f"p{i}: text/order differs ({len(x)} vs {len(y)} words)")
            continue
        d = max((max(abs(p - q) for p, q in zip(u[1], v[1])) for u, v in zip(x, y)), default=0.0)
        if d > TOL:
            out.append(f"p{i}: word box moved {d:.3f} pt")
    return out


def pypdf_text(path):
    r = pypdf.PdfReader(path)
    return ["".join(p.extract_text().split()) for p in r.pages]


def main(argv):
    inp, out = argv[0], argv[1]
    pw = argv[argv.index("--password") + 1] if "--password" in argv else None
    pending = "--pending" in argv
    tmp = None
    if pw is not None:
        tmp = tempfile.mkdtemp()
        plain = os.path.join(tmp, "plain.pdf")
        pikepdf.open(inp, password=pw).save(plain, encryption=False)
        inp = plain
    res = {"objects": objects.check_isolated(out, {"forbidden": [], "allow": []})}
    fid, notes = fidelity.check_isolated(inp, out, {"changed": [], "preserve": []})
    res["fidelity"] = fid
    res["fidelity_notes"] = notes
    res["words_fitz"] = words_findings(fitz_words(inp), fitz_words(out))
    res["words_pypdf"] = [] if pypdf_text(inp) == pypdf_text(out) else ["pypdf text differs"]
    if pending:  # annotation text may legitimately vanish; the loss must still be visible to the oracles
        res["pending_loss_seen"] = bool(fid or res["words_fitz"] or res["words_pypdf"])
    sys.stdout.flush()
    print("\n@@" + json.dumps(res))


if __name__ == "__main__":
    main(sys.argv[1:])
