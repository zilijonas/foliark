#!/usr/bin/env python3
"""(Re)build tests/fixtures/manifest.json and tests/corpus/public-manifest.json (NEW-6).

  python3 build_manifest.py [--hddn ~/ai/repos/hddn]   # HDDN checkout, READ-ONLY, content pinned to 2fe0f9a by sha256

Copies HDDN's synthetic fixtures into tests/fixtures/hddn/ when absent (sha256 must equal HDDN's own manifest),
then writes both manifests. Needs PyMuPDF (page counts only).
"""
import hashlib
import json
import pathlib
import shutil
import sys

import fitz

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import generate  # noqa: E402

FX = HERE.parent
CORPUS = FX.parent / "corpus"
PIN = "2fe0f9a"
NOT_IMPORTED = {"many-pages-1001": "302,476 B exceeds the 200 KB commit cap; replaced by generated/many-pages-1001 (156 KB, shared content stream)"}
HDDN_FEATURES = {
    "metadata-xmp": ["metadata", "xmp"], "annotation-comment": ["annotation"], "attachment": ["embedded-file"],
    "actions-js": ["javascript", "uri-action"], "acroform": ["form"], "optional-content-layer": ["ocg"],
    "hidden-ocr-text": ["invisible-text"], "thumbnail": ["thumbnail", "image"], "outline-bookmark": ["outline"],
    "actualtext": ["actualtext", "structure-alt"], "incremental-revision": ["incremental-update"],
    "shared-image": ["shared-image", "image", "multi-page"], "shared-vector": ["shared-vector", "multi-page"],
    "rotation-crop": ["rotation-crop", "multi-page"], "unicode-glyph-widths": ["non-bmp", "ligature", "embedded-font"],
    "encrypted": ["encrypted"], "malformed-truncated": ["truncated", "malformed"], "malformed-xref": ["malformed-xref", "xref-repair"],
    "oversized-image": ["dimension-bomb", "image"], "compressed-stream": ["flate-bomb", "decode-ratio"],
}
KEEP = ("mustKeepText", "mustRemove", "targets", "renderedTextRegions", "mustKeepRenderedText", "checks", "graphicSecret", "imageSecrets",
        "inputPolicy", "doNotRender", "doNotExtract")


def sha(b):
    return hashlib.sha256(b).hexdigest()


def pages(data, pw=None):
    fitz.TOOLS.mupdf_display_errors(False)
    try:
        d = fitz.open("pdf", data)
        if d.needs_pass and not d.authenticate(pw or ""):
            return None
        return d.page_count
    except Exception:
        return None


def main():
    hddn = pathlib.Path(sys.argv[sys.argv.index("--hddn") + 1] if "--hddn" in sys.argv else "~/ai/repos/hddn").expanduser()
    src = json.load(open(hddn / "tests/pdf/fixtures/manifest.json"))
    (FX / "hddn").mkdir(exist_ok=True)
    entries, skipped = [], []
    for h in src["fixtures"]:
        i = h["id"]
        if i in NOT_IMPORTED:
            skipped.append({"id": i, "reason": NOT_IMPORTED[i], "origin": "hddn@%s:tests/pdf/fixtures/%s" % (PIN, h["file"]), "sha256": h["sha256"], "bytes": h["bytes"]})
            continue
        dst = FX / "hddn" / h["file"]
        if not dst.exists():
            shutil.copyfile(hddn / "tests/pdf/fixtures" / h["file"], dst)
        data = dst.read_bytes()
        assert sha(data) == h["sha256"], i
        lic = "synthetic, generated (HDDN, owner-authored)" + ("; embeds DejaVu Sans glyphs (DejaVu license, free redistribution)" if b"DejaVu" in data else "")
        e = {"id": i, "path": "hddn/" + h["file"], "sha256": sha(data), "bytes": len(data), "pages": h.get("pageCount") and pages(data, h.get("password")),
             "features": HDDN_FEATURES[i], "origin": "hddn@%s:tests/pdf/fixtures/%s" % (PIN, h["file"]), "license": lic,
             "expected": {k: h[k] for k in KEEP if k in h}}
        if "password" in h:
            e["password"], e["ownerPassword"] = h["password"], h["ownerPassword"]
        entries.append(e)
    shutil.copyfile(hddn / "scripts/pdf-fixtures/generate.py", HERE / "hddn_generate_REFERENCE.py")
    for i, (fn, feats, exp, meta) in generate.FIXTURES.items():
        data = (FX / "generated" / (i + ".pdf")).read_bytes()
        e = {"id": i, "path": "generated/%s.pdf" % i, "sha256": sha(data), "bytes": len(data), "pages": pages(data, meta.get("password")),
             "features": feats, "origin": "tests/fixtures/generators/generate.py", "license": "synthetic, generated (original)",
             "deterministic": meta.get("deterministic", True), "expected": exp}
        for k in ("password", "ownerPassword"):
            if k in meta:
                e[k] = meta[k]
        entries.append(e)
    by_feature = {}
    for e in entries:
        for f in e["features"]:
            by_feature[f] = by_feature.get(f, 0) + 1
    man = {"schemaVersion": 1, "purpose": "Rights-aware fixture corpus: synthetic only. Third-party PDFs are never committed (tests/corpus/public-manifest.json).",
           "hddnPin": PIN, "pins": {"PyMuPDF": "1.26.5 (encryption + page counts, dev-only)", "python": "3.9"},
           "counts": {"total": len(entries), "hddn": sum(e["path"].startswith("hddn/") for e in entries),
                      "generated": sum(e["path"].startswith("generated/") for e in entries), "byFeature": dict(sorted(by_feature.items()))},
           "notImported": skipped, "files": entries}
    (FX / "manifest.json").write_text(json.dumps(man, indent=1, sort_keys=False) + "\n")

    pub = json.load(open(hddn / "tests/pdf/corpus/manifest.json"))
    files = [{"id": f["id"], "url": f["sourceUrl"], "sha256": f["sha256"], "bytes": f["bytes"], "license": f["license"],
              "redistribution": f["redistribution"], "permissionEvidence": f["permissionEvidence"], "sourceRepository": f["sourceRepository"],
              "sourceRevision": f["sourceRevision"], "classes": f["classes"], "languages": f["languages"], "family": f["family"],
              "origin": "hddn@%s:tests/pdf/corpus/manifest.json" % PIN, "fetchOnly": True, "committed": False}
             for f in pub["files"]]
    note = ("Fetch-only: no PDF bytes of these documents may be committed to Foliark. Fetch with HDDN's scripts/pdf-fixtures/fetch-corpus.py "
            "(pinned revision, verifies sha256) into a directory outside the repo. License column is upstream's claim as recorded by HDDN-4, not legal advice; "
            "mixed-multipage and the two signed samples have unresolved/unconfirmed terms.")
    CORPUS.mkdir(exist_ok=True)
    (CORPUS / "public-manifest.json").write_text(json.dumps({"schemaVersion": 1, "note": note, "files": files}, indent=1) + "\n")
    print(json.dumps(man["counts"]))


if __name__ == "__main__":
    main()
