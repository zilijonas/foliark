#!/usr/bin/env python3
"""Judge the files HDDN's own UI exported (E2E_EXPORT_DIR of hddn-foliark `pnpm run test:e2e:foliark`) with foliark's independent oracles.

  tests/oracles/.venv/bin/python tools/hddn-acceptance/check-exports.py EXPORT_DIR HDDN_FIXTURES_DIR

Never imports foliark. Each case runs objects.py (privacy) and fidelity.py (what else changed) on the exported file, and the same objects
check on the INPUT as a negative control (it must flag the secret there, or the check proves nothing). Exit 0 iff every case passes."""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "tests", "oracles"))
import fitz  # noqa: E402
import fidelity  # noqa: E402
import objects  # noqa: E402

out_dir, fix_dir = sys.argv[1], sys.argv[2]


def words(path, wanted):
    d = fitz.open(path)
    return [(i, tuple(w[:4])) for i, p in enumerate(d) for w in p.get_text("words") if w[4] in wanted]


def blocks_with(path, needle):
    d = fitz.open(path)
    return [(i, tuple(b[:4])) for i, p in enumerate(d) for b in p.get_text("blocks") if needle in b[4]]


def case(name, fixture, produced, forbid, regions, raster=False, allow=("metadata",)):
    src, out = os.path.join(fix_dir, fixture + ".pdf"), os.path.join(out_dir, produced)
    if not os.path.exists(out):
        return ["MISSING: %s was not produced by the e2e run" % produced]
    spec = {"forbidden": forbid, "allow": list(allow)}
    findings = ["objects " + f for f in objects.check_isolated(out, spec)]
    control = objects.check_isolated(src, {"forbidden": forbid, "allow": []})
    if not any(f.startswith(("TEXT_", "BYTES_", "STREAM")) for f in control):
        findings.append("CONTROL: the INPUT was not flagged, so the check proves nothing: %s" % control)
    fspec = {"changed": [{"page": p, "rect": list(r)} for p, r in regions]}
    if raster:
        fspec.update(raster=True, threshold=64, max_frac=0.01)
    f_findings, _ = fidelity.check_isolated(src, out, fspec)
    findings += ["fidelity " + f for f in f_findings]
    print("%-12s %s %s (%d regions, control flagged %d)" % (name, "PASS" if not findings else "FAIL", produced, len(regions), len(control)))
    for f in findings:
        print("   ", f[:300])
    return findings


bad = []
S = os.path.join(fix_dir, "multicolumn-unicode.pdf")
# Needles are whole strings: a short digit run such as "34567" is also a legal UTF-16 cmap range inside an embedded TrueType program
# (objects.py STREAM finding on font bytes), which is not text.
secret = ["rasa.kazlauskaite@example.com", "+370", "612", "34567", "Žemaitis", "Gediminas"]
bad += case("native", "multicolumn-unicode", "native.pdf", ["rasa.kazlauskaite@example.com", "+370 612 34567", "Žemaitis"],
            words(S, set(secret)))
E = os.path.join(fix_dir, "edit-paragraphs.pdf")
# "Confirm all markings" also confirms the auto-detected name, so the name and the keyword are the declared redactions; the edited paragraph is the declared edit.
bad += case("edit+redact", "edit-paragraphs", "edit-redact.pdf", ["Šiaulių", "ąžuolų", "Žemaitis"],
            blocks_with(E, "Šiaulių") + words(E, {"Helvetica", "Žemaitis", "Gediminas"}))
P = os.path.join(fix_dir, "pattern-secret.pdf")
bad += case("raster", "pattern-secret", "raster.pdf", [w[4] for w in fitz.open(P)[0].get_text("words") if "@" in w[4]],
            words(P, {w[4] for w in fitz.open(P)[0].get_text("words") if "@" in w[4]}), raster=True)
sys.exit(1 if bad else 0)
