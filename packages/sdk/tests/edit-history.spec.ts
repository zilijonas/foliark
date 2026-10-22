// Edit history and the privacy-wins order over the real wasm (spec 4, 5; NEW-53): edits replay from the source,
// text redactions resolve against the base revision, export runs edits, then redactions, then sanitization.
// Exported bytes are judged by independent readers (the objects oracle, PyMuPDF, pikepdf); the Rust suite
// (crates/foliark-engine/tests/edit_history.rs) covers the same contract with a synthetic page and exact bytes.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, afterEach, describe, expect, it } from "vitest";
import init from "../wasm/foliark_wasm.js";
import { createFoliark, isCurrentPreview, type DocumentHandle, type Foliark, type Rect } from "../src/index.js";
import { createWorkerCore } from "../src/worker-core.js";

const root = new URL("../../../", import.meta.url).pathname;
const wasmBytes = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const pdf = new Uint8Array(readFileSync(new URL("tests/fixtures/hddn/shared-image.pdf", `file://${root}`)));
const sans = new Uint8Array(readFileSync(new URL("crates/foliark-text/tests/data/LiberationSans-Regular.ttf", `file://${root}`)));
const python = join(root, "tests/oracles/.venv/bin/python");
const scratch = mkdtempSync(join(tmpdir(), "foliark-edit-history-"));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

const worker = (): Worker => {
  const { port1, port2 } = new MessageChannel();
  const core = createWorkerCore({
    post: (m, transfer = []) => port2.postMessage(m, { transfer }),
    loadWasm: () => init({ module_or_path: wasmBytes }).then(() => undefined),
  });
  port2.onmessage = (e) => core.onMessage(e.data);
  return Object.assign(port1, { terminate: () => (port1.close(), port2.close()) }) as unknown as Worker;
};
const clients: Foliark[] = [];
afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.dispose()));
});

const SECRET = "SECRET_SHARED_IMAGE";
const KEEP = "KEEP_PUBLIC_SHARED_IMAGE_P1";
async function open() {
  const f = await createFoliark({ wasmUrl: "unused", worker });
  clients.push(f);
  await f.registerFonts([{ bytes: sans.slice(), family: "Liberation Sans", weight: 400, style: "normal" }]);
  const doc = await f.open(pdf.slice());
  const { x, y, width, height } = (await doc.extractText(0)).words.find((w) => w.text === SECRET)!.box;
  const rects: Rect[] = [{ x: x - 1, y: y - 1, width: width + 2, height: height + 2 }];
  return { doc, rects };
}
const edit = (id: string, rects: Rect[], text: string) => ({ id, pageIndex: 0, rects, text });
const exportBytes = async (doc: DocumentHandle) => doc.export(await doc.prepareExport({ mode: "native" }));

/** The independent readers: PyMuPDF text (whitespace removed), the objects oracle's findings, Liberation Sans Type0 count. */
function readers(name: string, bytes: Uint8Array, forbid: string[]) {
  const file = join(scratch, name);
  writeFileSync(file, bytes);
  const code =
    "import sys, json, fitz, pikepdf\n" +
    "pdf = pikepdf.open(sys.argv[1])\n" +
    "fonts = sum(1 for o in pdf.objects if isinstance(o, pikepdf.Dictionary) and o.get('/Subtype') == '/Type0' and 'LiberationSans' in str(o.get('/BaseFont')))\n" +
    "print(json.dumps({'text': ''.join(p.get_text() for p in fitz.open(sys.argv[1])), 'fonts': fonts}))\n";
  const parsed = JSON.parse(execFileSync(python, ["-c", code, file], { cwd: root, encoding: "utf8" }).trim().split("\n").pop()!);
  let findings = "";
  try {
    execFileSync(python, ["tests/oracles/objects.py", file, ...forbid.flatMap((f) => ["--forbid", f])], { cwd: root, encoding: "utf8" });
  } catch (e) {
    findings = String((e as { stdout?: string }).stdout ?? e);
  }
  return { text: (parsed.text as string).replace(/\s+/g, ""), fonts: parsed.fonts as number, findings };
}
const squash = (s: string) => s.replace(/\s+/g, "");

describe("edits and redactions: privacy wins at export", () => {
  it("edit then redact the new word: the word is gone everywhere, the rest of the edit stays", async () => {
    const { doc, rects } = await open();
    expect((await doc.applyChanges({ baseRevision: 0, textEdits: [edit("e1", rects, "Fresh CODENAME text")] })).accepted).toBe(true);
    const [hit] = await doc.search("CODENAME");
    expect(hit.revision).toBe(1);
    const r = await doc.applyChanges({ baseRevision: 1, redactions: [{ kind: "text", id: "r1", pageIndex: 0, start: hit.start, end: hit.end, fill: null }] });
    expect(r).toMatchObject({ accepted: true, revision: 2 });
    const plan = await doc.prepareExport({ mode: "native" });
    expect(plan.report.edits.applied).toBe(1);
    expect(plan.report.redaction.clustersRemoved).toBeGreaterThanOrEqual(8);
    const out = readers("edit-then-redact.pdf", await doc.export(plan), ["CODENAME", SECRET]);
    expect(out.findings).toBe("");
    expect(out.text).toContain(squash("Fresh"));
    expect(out.text).toContain(squash("text"));
    expect(out.text).toContain(KEEP);
    expect(out.text).not.toContain("CODENAME");
    expect(out.text).not.toContain(SECRET);
  });

  it("a redaction stored before an edit of its words refuses the edit; the words stay gone", async () => {
    const { doc, rects } = await open();
    const [hit] = await doc.search(SECRET);
    const red = { kind: "text" as const, id: "r1", pageIndex: 0, start: hit.start, end: hit.end, fill: null };
    expect((await doc.applyChanges({ baseRevision: 0, redactions: [red] })).accepted).toBe(true);
    const refused = await doc.applyChanges({ baseRevision: 1, textEdits: [edit("e1", rects, "Replacement words")] });
    expect(refused).toMatchObject({ accepted: false, revision: 1, rejected: [{ id: "r1", details: { feature: "redactionTargetEdited" } }] });
    expect(doc.revision).toBe(1);
    const out = readers("refused.pdf", await exportBytes(doc), [SECRET, "Replacement"]);
    expect(out.findings).toBe("");
    expect(out.text).not.toContain(SECRET);
    expect(out.text).not.toContain("Replacement");
    expect(out.text).toContain(KEEP);

    // Same ChangeSet: the redaction resolves at the base text, so it names the words the edit replaces.
    const same = await doc.applyChanges({ baseRevision: 1, textEdits: [edit("e1", rects, "Replacement words")], redactions: [{ ...red, id: "r2" }] });
    expect(same).toMatchObject({ accepted: false, rejected: [{ id: "r2", details: { feature: "redactionTargetEdited" } }] });
  });

  it("edit-only export removes the old words without any redaction", async () => {
    const { doc, rects } = await open();
    await doc.applyChanges({ baseRevision: 0, textEdits: [edit("e1", rects, "Totally new words")] });
    const plan = await doc.prepareExport({ mode: "native" });
    expect([plan.report.edits.applied, plan.report.redaction.regions, plan.report.redaction.clustersRemoved]).toEqual([1, 0, 0]);
    const out = readers("edit-only.pdf", await doc.export(plan), [SECRET]);
    expect(out.findings).toBe("");
    expect(out.text).toContain(squash("Totally new words"));
    expect(out.text).not.toContain(SECRET);
  });
});

describe("history", () => {
  it("undo restores text, render and export exactly (sanitize-only export is the baseline)", async () => {
    const baseline = await open();
    const srcText = (await baseline.doc.extractText(0)).text;
    const srcRender = await baseline.doc.renderPage(0, { scale: 1.5 });
    const srcPlan = await baseline.doc.prepareExport({ mode: "native" });
    expect([srcPlan.report.edits.applied, srcPlan.report.redaction.regions]).toEqual([0, 0]);
    const srcExport = await baseline.doc.export(srcPlan);

    const { doc, rects } = await open();
    await doc.applyChanges({ baseRevision: 0, textEdits: [edit("e1", rects, "A different line of words")] });
    expect((await doc.extractText(0)).text).not.toBe(srcText);
    expect(Buffer.compare(Buffer.from((await doc.renderPage(0, { scale: 1.5 })).data), Buffer.from(srcRender.data))).not.toBe(0);
    expect(await doc.applyChanges({ baseRevision: 1, textEdits: [] })).toMatchObject({ accepted: true, revision: 2, changedPages: [0] });
    expect((await doc.extractText(0)).text).toBe(srcText);
    expect(Buffer.compare(Buffer.from((await doc.renderPage(0, { scale: 1.5 })).data), Buffer.from(srcRender.data))).toBe(0);
    expect(Buffer.compare(Buffer.from(await exportBytes(doc)), Buffer.from(srcExport))).toBe(0);
  });

  it("ten edits and undos keep one font object in the export and a deterministic size", async () => {
    const { doc, rects } = await open();
    const sizes: number[] = [];
    for (let i = 0; i < 10; i++) {
      expect(await doc.applyChanges({ baseRevision: doc.revision, textEdits: [edit("e1", rects, `Edit number ${i} ąčę`)] })).toMatchObject({ accepted: true });
      sizes.push((await exportBytes(doc)).byteLength);
      if (i % 2) {
        expect((await doc.applyChanges({ baseRevision: doc.revision, textEdits: [] })).accepted).toBe(true);
      }
    }
    expect(doc.revision).toBe(15);
    // Replays start from the source: the size follows the text, it does not accumulate.
    expect(Math.max(...sizes) - Math.min(...sizes)).toBeLessThan(2_000);
    await doc.applyChanges({ baseRevision: doc.revision, textEdits: [edit("e1", rects, "Final ąčę")] });
    const out = readers("repeated.pdf", await exportBytes(doc), [SECRET]);
    expect(out.fonts).toBe(1);
    expect(out.findings).toBe("");
  });

  it("a later edit over earlier text replaces it (rects see the state after the earlier edits); ids cannot repeat", async () => {
    const { doc, rects } = await open();
    const r = await doc.applyChanges({ baseRevision: 0, textEdits: [edit("first", rects, "Primary draft"), edit("second", rects, "Second version")] });
    expect(r).toMatchObject({ accepted: true, revision: 1 });
    expect(r.layouts.map((l) => l.id)).toEqual(["first", "second"]);
    const out = readers("replaced.pdf", await exportBytes(doc), ["Primary", SECRET]);
    expect(out.findings).toBe("");
    expect(out.text).toContain(squash("Second version"));
    expect(out.text).not.toContain("Primary");
    const dup = await doc.applyChanges({ baseRevision: 1, textEdits: [edit("a", rects, "x"), edit("a", rects, "y")] });
    expect(dup).toMatchObject({ accepted: false, revision: 1, rejected: [{ id: "a", code: "INVALID_ARGUMENT" }] });
  });
});

describe("stale results", () => {
  it("older hits, bases, previews and plans are refused after a commit, and fresh reads see the new layout", async () => {
    const { doc, rects } = await open();
    const [oldHit] = await doc.search(SECRET);
    expect(oldHit.revision).toBe(0);
    const oldPreview = await doc.layoutEdit(0, rects, "x");
    expect(isCurrentPreview(oldPreview, doc)).toBe(true);
    const plan = await doc.prepareExport({ mode: "native" });

    expect(await doc.applyChanges({ baseRevision: 0, textEdits: [edit("e1", rects, "Brand new text")] })).toMatchObject({ accepted: true });

    expect(isCurrentPreview(oldPreview, doc)).toBe(false);
    await expect(doc.export(plan)).rejects.toMatchObject({ code: "STALE_PLAN" });
    const stale = { baseRevision: 0, redactions: [{ kind: "text" as const, id: "r", pageIndex: 0, start: oldHit.start, end: oldHit.end }] };
    await expect(doc.applyChanges(stale)).rejects.toMatchObject({ code: "STALE_REVISION" });
    expect(await doc.search(SECRET)).toEqual([]);
    const [fresh] = await doc.search("Brand");
    expect(fresh.revision).toBe(1);
    expect(fresh.quads).not.toEqual(oldHit.quads);
    const page = await doc.extractText(0);
    expect(page.revision).toBe(1);
    expect(page.words.some((w) => w.text === "Brand")).toBe(true);
    expect(page.words.some((w) => w.text === SECRET)).toBe(false);
    const preview = await doc.layoutEdit(0, rects, "x");
    expect([preview.revision, isCurrentPreview(preview, doc)]).toEqual([1, true]);
    expect(preview.fingerprint).not.toBe(oldPreview.fingerprint);
    // Selected again at the new revision, the same kind of ChangeSet commits.
    const ok = await doc.applyChanges({ baseRevision: 1, redactions: [{ kind: "text", id: "r", pageIndex: 0, start: fresh.start, end: fresh.end }] });
    expect(ok.accepted).toBe(true);
  });
});
