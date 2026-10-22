# Native export size, Foliark vs PyMuPDF on the same input and the same keyword redactions (NEW-83). Dev-only: PyMuPDF is the oracle.
#   tests/oracles/.venv/bin/python tools/bench/export-size.py [--check]      ($FOLIARK_EXPORT_BIN overrides the binary; needs target/release/examples/export_size,
#   built by `cargo build --release -p foliark-engine --example export_size` through the memory queue)
# Keyword = the most frequent word of 4+ letters in the file (ties alphabetical), so every fixture is judged on the same flow as the
# HDDN keyword export: every hit redacted with a black fill. PyMuPDF = apply_redactions + tobytes(garbage=4, deflate=True).
# --check compares Foliark's size with budgets.json `exportBytes` (exit 1 on a regression, a missing fixture or a refused export).
import collections, json, os, pathlib, re, subprocess, sys, tempfile
import fitz

ROOT = pathlib.Path(__file__).resolve().parents[2]
HDDN = ROOT / "tests/fixtures/hddn"
BIN = pathlib.Path(os.environ.get("FOLIARK_EXPORT_BIN", ROOT / "target/release/examples/export_size"))
FIXTURES = ["native-30p"] + [f"hddn/{n}" for n in (
    "metadata-xmp", "compressed-stream", "unicode-glyph-widths", "rotation-crop", "shared-image", "shared-vector",
    "outline-bookmark", "annotation-comment", "oversized-image")]

def source(fid, tmp):
    if fid.startswith("hddn/"):
        return HDDN / (fid[5:] + ".pdf")
    out = tmp / (fid + ".pdf")
    subprocess.run([sys.executable, str(ROOT / "tools/bench/make-native30.py"), str(out)], check=True)
    return out

def keyword(path):
    words = collections.Counter(w.lower() for p in fitz.open(path) for w in re.findall(r"[A-Za-z]{4,}", p.get_text()))
    return min(words, key=lambda w: (-words[w], w)) if words else None

def mupdf(path, word):
    d = fitz.open(path)
    for p in d:
        for r in p.search_for(word):
            p.add_redact_annot(r, fill=(0, 0, 0))
        p.apply_redactions()
    return len(d.tobytes(garbage=4, deflate=True))

def foliark(path, word, tmp):
    r = subprocess.run([str(BIN), str(path), str(tmp / "out.pdf"), word], capture_output=True, text=True)
    return int(re.search(r"out (\d+) B", r.stdout).group(1)) if r.returncode == 0 else None

def main():
    budgets = {b["id"]: b for b in json.loads((ROOT / "tools/bench/budgets.json").read_text()).get("exportBytes", [])}
    rows, bad = [], 0
    with tempfile.TemporaryDirectory() as t:
        tmp = pathlib.Path(t)
        for fid in FIXTURES:
            p = source(fid, tmp)
            w = keyword(p)
            f = foliark(p, w, tmp) if w else None
            m = mupdf(p, w) if w else None
            b = budgets.get(fid)
            status = "-"
            if "--check" in sys.argv and b:
                status = "ok" if f is not None and f <= b["max"] else "REGRESSION"
                bad += status != "ok"
            rows.append((fid, p.stat().st_size, w, f, m, status))
    if "--check" in sys.argv:
        bad += len(budgets.keys() - {r[0] for r in rows})
    print("| fixture | input B | keyword | Foliark B | PyMuPDF B | Foliark / PyMuPDF |" + (" budget |" if "--check" in sys.argv else ""))
    print("|---|---|---|---|---|---|" + ("---|" if "--check" in sys.argv else ""))
    for fid, i, w, f, m, s in rows:
        ratio = f"{f / m:.2f}" if f and m else "-"
        print(f"| {fid} | {i:,} | {w} | {f if f is not None else 'refused'} | {m if m is not None else '-'} | {ratio} |" + (f" {s} |" if "--check" in sys.argv else ""))
    sys.exit(1 if bad else 0)

main()
