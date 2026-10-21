#!/usr/bin/env python3
"""Exit 0 iff the fixture corpus is consistent with manifest.json (NEW-6).

Checks: every manifest file exists with matching sha256/bytes/pages; license + features present; counts match;
no file under tests/fixtures outside the manifest (scripts in generators/ and the manifest itself excepted);
nothing fetch-only committed (tests/corpus holds only the manifest, and no file under tests/ matches a fetch-only sha256);
committed files <= 200 KB. --regen additionally rebuilds every deterministic generated file and compares bytes.
Needs PyMuPDF for page counts (pip install -r tests/oracles/requirements.txt).
"""
import hashlib
import json
import pathlib
import sys

import fitz

FX = pathlib.Path(__file__).resolve().parent
TESTS = FX.parent
CAP = 200_000
errs = []


def err(m):
    errs.append(m)


def sha(b):
    return hashlib.sha256(b).hexdigest()


def pages(data, pw):
    fitz.TOOLS.mupdf_display_errors(False)
    try:
        d = fitz.open("pdf", data)
        return None if d.needs_pass and not d.authenticate(pw or "") else d.page_count
    except Exception:
        return None


man = json.load(open(FX / "manifest.json"))
pub = json.load(open(TESTS / "corpus" / "public-manifest.json"))
ids = [e["id"] for e in man["files"]]
if len(set(ids)) != len(ids):
    err("duplicate ids")
listed = set()
for e in man["files"]:
    p = FX / e["path"]
    listed.add(p.resolve())
    if not e.get("license"):
        err("%s: no license" % e["id"])
    if not e.get("features"):
        err("%s: no features" % e["id"])
    if not e.get("origin"):
        err("%s: no origin" % e["id"])
    if not p.is_file():
        err("%s: missing %s" % (e["id"], e["path"]))
        continue
    d = p.read_bytes()
    if sha(d) != e["sha256"]:
        err("%s: sha256 mismatch" % e["id"])
    if len(d) != e["bytes"]:
        err("%s: bytes %d != %d" % (e["id"], len(d), e["bytes"]))
    if len(d) > CAP:
        err("%s: %d B over the %d B cap" % (e["id"], len(d), CAP))
    if e["pages"] is not None and pages(d, e.get("password")) != e["pages"]:
        err("%s: page count mismatch" % e["id"])
    if "--regen" in sys.argv and e.get("deterministic"):
        sys.path.insert(0, str(FX / "generators"))
        import generate
        if generate.FIXTURES[e["id"]][0]() != d:
            err("%s: generator output differs from committed bytes" % e["id"])

c = man["counts"]
if (c["total"], c["hddn"], c["generated"]) != (len(ids), sum(e["path"].startswith("hddn/") for e in man["files"]), sum(e["path"].startswith("generated/") for e in man["files"])):
    err("counts do not match files")
feat = {}
for e in man["files"]:
    for f in e["features"]:
        feat[f] = feat.get(f, 0) + 1
if dict(sorted(feat.items())) != c["byFeature"]:
    err("byFeature counts do not match")

skip = {"manifest.json", "verify.py", "README.md"}
for f in sorted(FX.rglob("*")):
    if f.is_file() and f.resolve() not in listed and "generators" not in f.relative_to(FX).parts and f.name not in skip and f.name != ".DS_Store":
        err("file outside manifest: %s" % f.relative_to(FX))

fetch_only = {e["sha256"] for e in pub["files"]}
for e in pub["files"]:
    if e.get("fetchOnly") is not True or e.get("committed") is not False:
        err("public %s: must be fetchOnly=true committed=false" % e["id"])
    if not (e.get("url") and e.get("license")):
        err("public %s: url/license missing" % e["id"])
for f in TESTS.rglob("*"):
    if f.is_file() and ".venv" not in f.parts and "__pycache__" not in f.parts:
        if f.is_relative_to(TESTS / "corpus") and f.name != "public-manifest.json":
            err("tests/corpus must hold only the manifest: %s" % f.name)
        if f.stat().st_size and sha(f.read_bytes()) in fetch_only:
            err("fetch-only document committed: %s" % f)

print("fixtures: %d files (%d hddn, %d generated), %d public fetch-only entries, %d errors" % (c["total"], c["hddn"], c["generated"], len(pub["files"]), len(errs)))
for m in errs:
    print("FAIL", m)
sys.exit(1 if errs else 0)
