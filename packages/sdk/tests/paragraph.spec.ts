// Paragraph edits over the real wasm (spec 1.5, 4, 6): prepareParagraph, applyChanges(textEdits) and what extractText
// and export show afterwards. The Rust suite (crates/foliark-engine/tests/paragraph_edits.rs) judges the output bytes
// with independent readers; this proves the protocol, the transferred editor face and the revision bookkeeping.
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import init from "../wasm/foliark_wasm.js";
import { createFoliark, type Foliark } from "../src/index.js";
import { createWorkerCore } from "../src/worker-core.js";

const wasmBytes = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const pdf = new Uint8Array(readFileSync(new URL("../../../tests/fixtures/hddn/shared-image.pdf", import.meta.url)));
const wordPdf = new Uint8Array(readFileSync(new URL("../../../tests/fixtures/generated/word-bold-heading-body.pdf", import.meta.url)));
const alignedPdf = new Uint8Array(readFileSync(new URL("../../../tests/fixtures/generated/pdflib-right-aligned.pdf", import.meta.url)));
const sans = new Uint8Array(readFileSync(new URL("../../../crates/foliark-text/tests/data/LiberationSans-Regular.ttf", import.meta.url)));

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

describe("paragraph edits", () => {
  it("prepares a paragraph, replaces its text, and the new revision reads back the new words", async () => {
    const f = await createFoliark({ wasmUrl: "unused", worker });
    clients.push(f);
    const [font] = await f.registerFonts([{ bytes: sans.slice(), family: "Liberation Sans", weight: 400, style: "normal" }]);
    const doc = await f.open(pdf.slice());
    const secret = (await doc.extractText(0)).words.find((w) => w.text === "SECRET_SHARED_IMAGE")!;
    const { x, y, width, height } = secret.box;
    const rects = [{ x: x - 1, y: y - 1, width: width + 2, height: height + 2 }];

    const info = await doc.prepareParagraph(0, rects, { font: font.id, editorFace: true });
    expect(info).toMatchObject({ docId: doc.id, revision: 0, pageIndex: 0, originalText: "SECRET_SHARED_IMAGE", font: { source: "registered", fontId: font.id, rule: "caller" } });
    expect(info.editorFace!.bytes).toBeInstanceOf(Uint8Array);
    expect(info.editorFace!.bytes.byteLength).toBe(sans.byteLength);

    const r = await doc.applyChanges({ baseRevision: 0, textEdits: [{ id: "e1", pageIndex: 0, rects, text: "ąčęėįšųūž", font: font.id }] });
    expect(r).toMatchObject({ accepted: true, revision: 1, changedPages: [0] });
    expect(r.layouts).toEqual([{ id: "e1", lines: 1, approximate: true, fingerprint: expect.stringMatching(/^[0-9a-f]{16}$/), runs: [{ source: "registered", font: font.id, clusters: 9 }] }]);
    const page = await doc.extractText(0);
    expect(page.revision).toBe(1);
    expect(page.text).toContain("ąčęėįšųūž");
    expect(page.text).not.toContain("SECRET_SHARED_IMAGE");

    // An edit that cannot be set is a rejection that names the edit and changes nothing.
    const bad = await doc.applyChanges({ baseRevision: 1, textEdits: [{ id: "big", pageIndex: 0, rects, text: "word ".repeat(500) }] });
    expect(bad).toMatchObject({ accepted: false, revision: 1, rejected: [{ id: "big", code: "EDIT_NO_FIT", details: { editId: "big" } }] });

    const plan = await doc.prepareExport({ mode: "native" });
    expect(plan.report.edits).toEqual({ applied: 1, approximate: 1 });
    expect(new TextDecoder("latin1").decode((await doc.export(plan)).slice(0, 8))).toBe("%PDF-1.7");
  });

  it("reports the style segments of a mixed block, rejoining to its text", async () => {
    const f = await createFoliark({ wasmUrl: "unused", worker });
    clients.push(f);
    const doc = await f.open(wordPdf.slice());
    const info = await doc.prepareParagraph(0, [{ x: 15, y: 94, width: 165, height: 31 }], { editorFace: true });
    const text = info.originalText!;
    expect(info.segments.map((s) => text.slice(s.start, s.end)).join("")).toBe(text);
    expect(info.segments).toMatchObject([
      { start: 0, bold: true, font: { source: "embedded" } },
      { end: text.length, bold: false, font: { source: "embedded" } },
    ]);
    expect(text.slice(0, info.segments[0].end)).toBe("1 ARTICLE. SCOPE\n");
    expect(info.segments[1].editorFace!.bytes).toBeInstanceOf(Uint8Array);
    // ADR 0013: the looks (body, heading) and the runs of the text they draw, each face a Uint8Array.
    expect(info.runs.map((r) => text.slice(r.start, r.end)).join("")).toBe(text);
    expect(info.looks.map((l) => l.bold)).toEqual([false, true]);
    expect(info.runs[0]).toMatchObject({ start: 0, look: 1 });
    for (const l of info.looks) expect(l.editorFace!.bytes).toBeInstanceOf(Uint8Array);
    const p = await doc.layoutEdit(0, [{ x: 15, y: 94, width: 165, height: 31 }], text.replace("charlie", "charles"));
    expect(p.styles.map((s) => s.look)).toEqual(info.runs.map((r) => r.look));
  });

  it("reports a right-aligned block's alignment and lays its edit out flush to the same edge", async () => {
    // pdflib-right-aligned: the invoice block's rows end at x 280 (ADR 0019).
    const f = await createFoliark({ wasmUrl: "unused", worker });
    clients.push(f);
    const doc = await f.open(alignedPdf.slice());
    const rects = [{ x: 130, y: 30, width: 155, height: 28 }];
    const info = await doc.prepareParagraph(0, rects);
    expect(info).toMatchObject({ align: "right", originalText: "Invoice date: 2026-10-12\nDue by: 2026-11-12" });
    expect(info.originX + info.wrapWidth).toBeCloseTo(280, 3);
    const p = await doc.layoutEdit(0, rects, "Invoice date: 2026-12-21 to 2026\nDue by: 2026-11-12");
    expect(p.fits).toBe(true);
    expect(p.lines.map((l) => l.x + l.width)).toEqual([expect.closeTo(280, 3), expect.closeTo(280, 3)]);
  });
});
