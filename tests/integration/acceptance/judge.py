#!/usr/bin/env python3
"""Independent judge of the acceptance run (NEW-59). Never imports foliark; uses tests/oracles (pikepdf, pypdf, PyMuPDF).
  judge.py prep OUT      write OUT/cases.json = manifest fixtures + adversarial fixtures (generators/adversarial.py)
  judge.py judge OUT     judge OUT/results.json (written by drive.mjs): per case and scenario, then negative controls; exit 0 iff no failure
Buckets: SAFE (bytes exported and every oracle passes), REFUSED (typed refusal named by the case, no bytes), EXPLAINED (scenario cannot apply, reason
recorded), FAIL (anything else: leak, oracle finding, wrong refusal, crash, unexplained skip).
"""
import concurrent.futures as cf
import hashlib
import json
import os
import pathlib
import re
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO / "tests/oracles"))
sys.path.insert(0, str(HERE / "generators"))
sys.path.insert(0, str(REPO / "tests/integration/strict_native"))
import fitz  # noqa: E402
import fidelity  # noqa: E402
import objects  # noqa: E402
import pikepdf  # noqa: E402
import check as sn  # noqa: E402  (strict_native/check.py helpers: plain_copy, secret_rects, no_ap_rects, page_text)

REFUSED_FIXTURES = {"malformed-truncated": "REPAIRED_PDF", "malformed-xref": "REPAIRED_PDF", "oversized-image": "LIMIT_EXCEEDED", "compressed-stream": "LIMIT_EXCEEDED",
                    "corrupt-flate": "DECODE_FAILED", "forms-cyclic": "MALFORMED_PDF", "forms-deep-40": "LIMIT_EXCEEDED", "many-pages-1001": "LIMIT_EXCEEDED"}
RASTER_REFUSALS = {"REPAIRED_PDF", "MALFORMED_PDF", "DECODE_FAILED", "LIMIT_EXCEEDED", "RENDER_INCOMPLETE"}
ALL_ALLOW = ["metadata", "embeddedfiles", "javascript", "openaction", "aa", "acroform", "ocproperties", "info"]
flat = lambda s: "".join(s.split())
sha = lambda b: hashlib.sha256(b).hexdigest()


def prep(out):
    from adversarial import build_all
    out = pathlib.Path(out)
    cases = build_all(out / "adv")
    m = json.load(open(REPO / "tests/fixtures/manifest.json"))
    for f in m["files"]:
        e = f.get("expected", {})
        regs = [{"kind": r["kind"], "page": r["page"], "rect": r["rectTopLeft"], "text": r.get("text", "")} for r in e.get("regions", []) if r["kind"].startswith("sensitive")]
        for t in e.get("targets", []):
            w, h = t["displaySize"]
            b = t["box"]
            regs.append({"kind": "region", "page": t["pageIndex"], "rect": [b["x"] * w, b["y"] * h, (b["x"] + b["width"]) * w, (b["y"] + b["height"]) * h], "text": "HDDN target"})
        feats = f.get("features", [])
        code = REFUSED_FIXTURES.get(f["id"])
        cases.append({"id": f["id"], "path": str(REPO / "tests/fixtures" / f["path"]), "secrets": sorted(set(e.get("mustRemove", []))), "keep": e.get("mustKeepText", []) + e.get("mustKeepRenderedText", []),
                      "regions": regs, "expect": {s: code for s in ("sanitize", "region", "text", "edit")} if code else {}, "survivors": [], "sanitize_keep": [], "retained": [],
                      "hidden": True, "note": "manifest", "password": f.get("password"), "flags": {"layers": "ocg" in feats, "actualtext": "actualtext" in feats}, "manifest": True,
                      "refused": code})
    json.dump(cases, open(out / "cases.json", "w"), indent=1)
    print(len(cases), "cases")


def hexforms(s):
    raw = s.encode()
    return {raw.hex(), raw.hex().upper(), s.encode("utf-16-be").hex(), s.encode("utf-16-be").hex().upper(), "".join("\\%03o" % c for c in raw)}


def blob_scan(path, secrets):
    """Encoded forms the objects oracle does not look for: hex strings and octal escapes in file bytes and in every decoded stream."""
    out = []
    blobs = [open(path, "rb").read()]
    try:
        pdf = pikepdf.open(path)
        for o in pdf.objects:
            if isinstance(o, pikepdf.Stream):
                try:
                    blobs.append(o.read_bytes())
                except Exception:
                    pass
    except Exception as e:  # noqa: BLE001
        return ["OPEN: pikepdf %s" % e]
    for s in secrets:
        for form in hexforms(s):
            fb = form.encode()
            if any(fb in b or (b"\\" not in fb and fb.lower() in b.lower()) for b in blobs):
                out.append("ENCODED: %r found as %s" % (s, form[:24]))
                break
    return out


def judge_file(case, scen, rec, out_path, inp, doc, visible):
    """All findings for one exported file (empty = pass)."""
    secrets = case["secrets"]
    survivors, retained, skeep = set(case["survivors"]), set(case["retained"]), set(case["sanitize_keep"])
    raster = scen.startswith("raster")
    regions = [{"page": r[0], "rect": r[1:]} for r in rec.get("regions", [])]
    declared = [g for g in case["regions"] if g["kind"].startswith("sensitive") or g["kind"] == "region"]
    # image erasure is a closed-set test (a sample touching a region edge is erased too): the declared change of an image region grows by one sample
    grow = lambda g: [g["rect"][0] - 4, g["rect"][1] - 4, g["rect"][2] + 4, g["rect"][3] + 4] if g["kind"] == "sensitive-image" else g["rect"]
    if scen in ("sanitize", "raster_sanitize"):
        forbid = [s for s in secrets if (raster or flat(s) not in flat(visible)) and s not in skeep and s not in retained and s not in survivors]
        if raster:
            forbid = [s for s in secrets if flat(s) not in flat(visible)]
        if case["flags"].get("must_gone"):  # the oracle renderer shows it (it ignores /VE); the spec'd reading says hidden, so it must be gone
            forbid = list(secrets)
        changed = sn.no_ap_rects(doc)
    else:
        forbid = [s for s in secrets if s not in survivors and (raster or s not in retained)]
        if scen == "edit":
            forbid = [rec["retyped"]]
        rects = [{"page": g[0], "rect": [g[1], g[2], g[3], g[4]]} for g in rec.get("regions", [])]
        if scen in ("region", "raster") or case["flags"].get("declared_changed"):
            rects += [{"page": g["page"], "rect": grow(g)} for g in declared]
        if scen == "edit":
            e = rec["edit_rect"]
            rects.append({"page": e[0], "rect": [e[1] - 4, e[2] - 4, e[3] + 4, e[4] + 4]})
        changed = rects + sn.secret_rects(doc, secrets) + sn.no_ap_rects(doc)
    spec = {"forbidden": forbid, "password": ""}
    if not raster and scen == "region":
        spec["image_regions"] = [{"page": g["page"], "rect": g["rect"], "color": g.get("color", [0, 0, 0])} for g in case["regions"] if g["kind"] == "sensitive-image"]
        spec["vector_regions"] = [{"page": g["page"], "rect": g["rect"], "fill": [0, 0, 0]} for g in case["regions"] if g["kind"] == "sensitive-vector"]
    if raster and regions:  # every pixel the region touches is burned; the oracle tests squares touching the rect, so inset by under half a pixel
        spec["image_regions"] = [{"page": g["page"], "rect": [g["rect"][0] + .26, g["rect"][1] + .26, g["rect"][2] - .26, g["rect"][3] - .26], "color": [0, 0, 0]} for g in regions if g["rect"][2] - g["rect"][0] > 1 and g["rect"][3] - g["rect"][1] > 1]
    occ = case["flags"].get("occ")
    if occ and scen in ("region",):
        spec["occurrences"] = [{"page": 0, "path": p, "forbid": [secrets[0]]} if k == "forbid" else {"page": 0, "path": p, "require": [secrets[0]]} for p, k in occ]
    f = list(objects.check_isolated(out_path, spec))
    f += blob_scan(out_path, forbid)
    if case.get("password") is None or True:
        keep = [k for k in case["keep"]]
        if scen == "edit":
            keep = [k for k in keep if not k.startswith("KEEP_PUBLIC")]
        fspec = {"changed": changed, "preserve": [] if raster else keep}
        if raster:
            fspec.update(raster=True, threshold=64, max_frac=0.01)
        if raster and case.get("password"):
            fid = []
        else:
            fid, _ = fidelity.check_isolated(inp, out_path, fspec)
        if case["flags"].get("layers"):
            fid = [x for x in fid if not x.startswith("WORDS_PYPDF")]
        if case["flags"].get("actualtext"):
            fid = [x for x in fid if not x.startswith("WORDS_")]
        if case["flags"].get("fid_blind"):  # the oracle renderer ignores /VE, so its pixel, glyph and word views of the input are wrong for this case
            fid = [x for x in fid if not x.startswith(("PIXELS", "GLYPHS", "WORDS_"))]
        if case["flags"].get("pypdf_blind"):  # pypdf misplaces text shown by ' and " (implicit line move); PyMuPDF's word check stays authoritative
            fid = [x for x in fid if not x.startswith("WORDS_PYPDF")]
        if case["flags"].get("offpage") and scen == "sanitize":  # clipped-out / off-CropBox content is removed by design (spec 8.2 offPage)
            fid = [x for x in fid if not (x.startswith("WORDS_PYPDF") and set(re.findall(r"\('([^']*)', ", x)) <= set(secrets))]
        fid = [x for x in fid if not (x.startswith("WORDS_PYPDF") and set(re.findall(r"\('([^']*)', ", x)) <= set(secrets) and "text chunks missing" in x and scen != "sanitize")]
        f += fid
    # text still readable under any redacted region (PyMuPDF clip), for every secret (survivors included: they must not survive INSIDE a region)
    if not raster and scen in ("region", "edit", "text") and rec.get("regions"):
        out = fitz.open(out_path)
        if out.needs_pass:
            out.authenticate("")
        for g in rec["regions"]:
            page = out[g[0]]
            clip = fitz.Rect(g[1:]) * page.derotation_matrix
            t = flat(page.get_text("text", clip=clip))
            f += ["REGION_TEXT: %r readable inside redacted region %s" % (s, [round(v) for v in g[1:]]) for s in (secrets if scen != "edit" else [rec["retyped"]]) if flat(s) in t]
    return f


def judge_task(args):
    e = judge_one(args)
    known = json.load(open(HERE / "known_blockers.json")).get("%s/%s" % (e["case"], e["scenario"]))
    if known and e["bucket"] == "FAIL":
        e.update(bucket="BLOCKER", detail="%s: %s" % (known, e["detail"]))
    elif known and e["bucket"] != "FAIL":
        e.update(bucket="FAIL", detail="%s is listed as an unresolved blocker but passes now: remove it from known_blockers.json" % known)
    return e


def judge_one(args):
    case, scen, rec, root = args
    exp = case["expect"].get(scen)
    pdfp = str(pathlib.Path(root) / case["id"] / (scen + ".pdf"))
    entry = {"case": case["id"], "scenario": scen, "mode": "raster" if scen.startswith("raster") else "native", "in": rec["input_sha256"][:12], "out": "", "bucket": "", "detail": ""}
    o = rec["scenarios"][scen] if "scenarios" in rec else rec
    if o["outcome"] == "crash":
        entry.update(bucket="FAIL", detail="crash: " + o["message"][:200])
    elif o["outcome"] == "n/a":
        r = o["reason"]
        ok = (case["hidden"] or not case["secrets"] or (case["survivors"] and scen == "text")) or scen in ("raster_sanitize",)
        entry.update(bucket="EXPLAINED" if ok else "FAIL", detail=("n/a: " if ok else "UNEXPLAINED skip: ") + r)
    elif o["outcome"] == "refused":
        code = o["code"]
        refused = case.get("refused")
        if scen.startswith("raster"):  # spec 7: a page that does not render complete is a typed refusal (annotation-comment is pinned by NEW-56)
            good = code == "RENDER_INCOMPLETE" or code == exp or code == case["expect"].get("sanitize") or (refused and code in RASTER_REFUSALS)
        elif scen == "edit":
            good = code == exp or code.startswith("EDIT_") or (refused and code == refused)
        else:
            good = code == exp
        entry.update(bucket="REFUSED" if good else "FAIL", detail=("typed %s" % code if good else "unexpected refusal %s %s" % (code, json.dumps(o.get("details")))))
        if os.path.exists(pdfp):
            entry.update(bucket="FAIL", detail="refused but bytes exist")
    else:
        entry["out"] = o["sha256"][:12]
        if exp and not scen.startswith("raster"):
            entry.update(bucket="FAIL", detail="expected refusal %s but exported %d bytes" % (exp, o["bytes"]))
            return entry
        if case.get("refused") and not scen.startswith("raster"):
            entry.update(bucket="FAIL", detail="refused fixture exported bytes")
            return entry
        tmp = tempfile.mkdtemp()
        inp = sn.plain_copy(case["path"], case["password"], tmp)
        ref = objects.named_copy(inp, os.path.join(tmp, "named.pdf"))  # outputs are judged against the input as its glyphs read (check.py)
        doc = fitz.open(ref)
        visible = sn.page_text(doc)
        # the input control: every secret must be visible to the oracle in the input, else a clean output proves nothing
        ctrl = []
        if case["secrets"] and scen in ("sanitize", "region"):
            found = objects.check_isolated(inp, {"forbidden": case["secrets"], "allow": ALL_ALLOW})
            ctrl = ["INPUT_CONTROL: %r not found in the input by objects.py" % s for s in case["secrets"] if not any(repr(s) in x for x in found)]
        f = ctrl + judge_file(case, scen, o, pdfp, ref, doc, visible)
        entry.update(bucket="FAIL" if f else "SAFE", detail="; ".join(f)[:600])
        if sha(open(pdfp, "rb").read()) != o["sha256"]:
            entry.update(bucket="FAIL", detail="output file hash differs from the driver record")
    return entry


def control_fake_overlay(case, rec_region, out_dir):
    """Leaky variant: the INPUT with black rectangles appended over the regions (a fake redaction). The judge must flag it."""
    inp = sn.plain_copy(case["path"], case["password"], out_dir)
    pdf = pikepdf.open(inp)
    for g in rec_region["regions"]:
        page = pdf.pages[g[0]]
        r = fitz.Rect(g[1:]) * fitz.open(inp)[g[0]].derotation_matrix
        mb = [float(v) for v in page.MediaBox]
        cs = pikepdf.Stream(pdf, ("q 0 g %f %f %f %f re f Q\n" % (r.x0, mb[3] - r.y1, r.width, r.height)).encode()) if int(page.get("/Rotate", 0)) == 0 and "/CropBox" not in page else None
        if cs is None:
            return None
        page.Contents = pikepdf.Array([*(page.Contents if isinstance(page.Contents, pikepdf.Array) else [page.Contents]), cs])
    path = os.path.join(out_dir, "fake-overlay.pdf")
    pdf.save(path)
    return path


def negative_controls(root, cases, res):
    """Every leaky variant must produce at least one finding from judge_file; a miss is a failure of the oracle stack."""
    rows = []
    by = {c["id"]: c for c in cases}
    for cid, r in res.items():
        c = by[cid]
        s = r["scenarios"].get("region", {})
        if s.get("outcome") != "ok" or not c["secrets"] or c["hidden"] and not c.get("manifest") is None and False:
            continue
        san = r["scenarios"].get("sanitize", {})
        tmp = tempfile.mkdtemp()
        inp = objects.named_copy(sn.plain_copy(c["path"], c["password"], tmp), os.path.join(tmp, "named.pdf"))
        doc = fitz.open(inp)
        visible = sn.page_text(doc)
        seen = [x for x in c["secrets"] if flat(x) in flat(visible) and x not in c["survivors"]]
        if not seen:
            continue
        # A: sanitize-only bytes presented as the redacted output
        sp = str(pathlib.Path(root) / cid / "sanitize.pdf")
        f = judge_file(c, "region", s, sp, inp, doc, visible) if san.get("outcome") == "ok" else None
        if f is not None and any(flat(x) in flat("".join(p.get_text() for p in fitz.open(sp))) for x in seen):  # only a variant that really still shows a secret is a valid leak
            rows.append({"case": cid, "control": "sanitize-bytes-as-redacted", "caught": bool(f), "finding": (f[0] if f else "")[:90]})
        # B: fake overlay (black boxes over live text)
        ov = control_fake_overlay(c, s, tmp)
        if ov:
            f = judge_file(c, "region", s, ov, inp, doc, visible)
            rows.append({"case": cid, "control": "fake-overlay", "caught": bool(f), "finding": (f[0] if f else "")[:90]})
    # C: injected hidden-data variants on one good output
    good = next(c for c in cases if c["id"] == "tj-kerning")
    base = pathlib.Path(root) / good["id"] / "region.pdf"
    s = res[good["id"]]["scenarios"]["region"]
    tmp = tempfile.mkdtemp()
    inp = sn.plain_copy(good["path"], None, tmp)
    doc = fitz.open(inp)
    sec = good["secrets"][0]
    variants = {}
    pdf = pikepdf.open(base); pdf.docinfo["/Title"] = sec; variants["secret-in-info"] = pdf
    pdf = pikepdf.open(base); pdf.Root.Hidden = pikepdf.Stream(pdf, ("<" + sec.encode().hex() + ">").encode()); variants["hex-in-stream"] = pdf
    pdf = pikepdf.open(base); pdf.Root.Pieces = pikepdf.Dictionary(Private=pikepdf.String(sec)); variants["secret-in-private-key"] = pdf
    pdf = pikepdf.open(base); variants["trailing-orphan-object"] = pdf
    pdf = pikepdf.open(base); pg = pdf.pages[0]; pg.Contents = pikepdf.Array([pg.Contents, pikepdf.Stream(pdf, ("BT /F1 10 Tf 5 5 Td [(%s) 20 (X)] TJ ET" % sec[:6]).encode() + b"\nBT /F1 10 Tf 5 25 Td (" + sec.encode() + b") Tj ET")]); variants["text-redrawn"] = pdf
    for name, p in variants.items():
        path = os.path.join(tmp, name + ".pdf")
        p.save(path, object_stream_mode=pikepdf.ObjectStreamMode.disable, deterministic_id=True)
        if name == "trailing-orphan-object":
            open(path, "ab").write(b"\n99 0 obj\n<< /Orphan (" + sec.encode() + b") >>\nendobj\n")
        f = judge_file({**good, "secrets": [sec]}, "region", s, path, inp, doc, sn.page_text(doc))
        rows.append({"case": good["id"], "control": name, "caught": bool(f), "finding": (f[0] if f else "")[:90]})
    return rows


def judge_cancel(root, cancel, cases):
    c = next(x for x in cases if x["id"] == "multipage-12")
    f = []
    for a in cancel["attempts"]:
        tag = "%s@%s" % (a["phase"], a["k"])
        if "error" in a:
            f.append("%s: error %s" % (tag, a["error"])); continue
        if a["first"] not in ("accepted", "rejected", "completed", "CANCELLED"):
            f.append("%s: unexpected first outcome %s" % (tag, a["first"]))
        data = open(a["file"], "rb").read()
        if sha(data) != a["sha256"]:
            f.append("%s: file/hash mismatch" % tag)
        if a["phase"] == "apply" and a.get("revision") == 0 and a["first"] != "accepted":
            # not applied: the export is the sanitize-only bytes (secrets still there), and re-applying must reproduce the clean run
            if a.get("reapplied_sha256") != cancel["baseline_sha256"]:
                f.append("%s: re-apply after a cancelled apply differs from a clean run" % tag)
        else:
            if a["sha256"] != cancel["baseline_raster_sha256" if a["phase"] == "raster" else "baseline_sha256"] and a["phase"] != "apply":
                f.append("%s: re-export after cancel differs from a clean run (stale bytes?)" % tag)
            if a["phase"] == "apply" and a["sha256"] != cancel["baseline_sha256"]:
                f.append("%s: accepted apply exports different bytes than a clean run" % tag)
            fnd = objects.check_isolated(a["file"], {"forbidden": c["secrets"], "password": ""})
            if fnd:
                f.append("%s: %s" % (tag, fnd[0][:120]))
    if not any(a.get("first") == "CANCELLED" for a in cancel["attempts"]):
        f.append("no attempt was ever cancelled: the cancellation scenario proves nothing")
    return f


def main_judge(root):
    root = pathlib.Path(root)
    cases = json.load(open(root / "cases.json"))
    data = json.load(open(root / "results.json"))
    res = data["results"]
    by = {c["id"]: c for c in cases}
    tasks = [(by[cid], scen, r, str(root)) for cid, r in res.items() for scen in r["scenarios"] for r in [dict(res[cid], scenarios=res[cid]["scenarios"])]]
    tasks = [(by[cid], scen, {"input_sha256": r["input_sha256"], "scenarios": r["scenarios"]}, str(root)) for cid, r in res.items() for scen in r["scenarios"]]
    entries = []
    with cf.ProcessPoolExecutor(max_workers=int(os.environ.get("JUDGE_JOBS", "4"))) as ex:
        for e in ex.map(judge_task, tasks, chunksize=2):
            entries.append(e)
    ctr = negative_controls(root, cases, res)
    cancel_f = judge_cancel(root, data["cancel"], cases) if data.get("cancel") else ["no cancellation run"]
    counts = {}
    for e in entries:
        counts[e["bucket"]] = counts.get(e["bucket"], 0) + 1
    with open(root / "RESULTS.tsv", "w") as fh:
        fh.write("case\tscenario\tmode\tbucket\tinput_sha12\toutput_sha12\tdetail\n")
        for e in entries:
            fh.write("\t".join([e["case"], e["scenario"], e["mode"], e["bucket"], e["in"], e["out"], e["detail"].replace("\t", " ")[:300]]) + "\n")
    json.dump({"entries": entries, "controls": ctr, "cancel_findings": cancel_f}, open(root / "judged.json", "w"), indent=1)
    fails = [e for e in entries if e["bucket"] == "FAIL"]
    for e in entries:
        if e["bucket"] == "BLOCKER":
            print("BLOCKER %s/%s: %s" % (e["case"], e["scenario"], e["detail"][:200]))
    for e in fails:
        print("FAIL %s/%s: %s" % (e["case"], e["scenario"], e["detail"]))
    missed = [c for c in ctr if not c["caught"]]
    for c in missed:
        print("NEGATIVE CONTROL MISSED %s %s" % (c["case"], c["control"]))
    for x in cancel_f:
        print("CANCEL FAIL", x)
    print("cases %d, scenario results %d: %s; negative controls %d (missed %d); cancellation attempts %d (findings %d)" % (len(cases), len(entries), counts, len(ctr), len(missed), len(data["cancel"]["attempts"]) if data.get("cancel") else 0, len(cancel_f)))
    sys.exit(1 if fails or missed or cancel_f else 0)


if __name__ == "__main__":
    {"prep": prep, "judge": main_judge}[sys.argv[1]](sys.argv[2])
