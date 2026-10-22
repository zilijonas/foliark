// Edit previews over the real wasm (ADR 0004): `layoutEdit` is the layout `applyChanges` commits, without committing.
// The Rust suite (crates/foliark-engine/tests/edit_preview.rs) covers boxes and refusals; this proves the protocol,
// the SDK surface and that the fingerprint a preview returns is the one the commit reports.
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import init from "../wasm/foliark_wasm.js";
import { createFoliark, FoliarkError, isCurrentPreview, type Foliark } from "../src/index.js";
import { createWorkerCore } from "../src/worker-core.js";

const wasmBytes = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const pdf = new Uint8Array(readFileSync(new URL("../../../tests/fixtures/hddn/shared-image.pdf", import.meta.url)));
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

async function open(fonts: boolean) {
  const f = await createFoliark({ wasmUrl: "unused", worker });
  clients.push(f);
  if (fonts) await f.registerFonts([{ bytes: sans.slice(), family: "Liberation Sans", weight: 400, style: "normal" }]);
  const doc = await f.open(pdf.slice());
  const { x, y, width, height } = (await doc.extractText(0)).words.find((w) => w.text === "SECRET_SHARED_IMAGE")!.box;
  return { f, doc, rects: [{ x: x - 1, y: y - 1, width: width + 2, height: height + 2 }] };
}
/** What `applyChanges` reports for the same inputs, on a fresh document each time (a commit changes the page). */
async function committed(text: string) {
  const { doc, rects } = await open(true);
  const r = await doc.applyChanges({ baseRevision: 0, textEdits: [{ id: "e", pageIndex: 0, rects, text }] });
  return r;
}

describe("layoutEdit", () => {
  it("types, deletes and breaks lines: every step previews what its commit reports, and nothing is committed", async () => {
    const { doc, rects } = await open(true);
    const before = (await doc.extractText(0)).text;
    const seen = new Map<string, string>();
    // append characters, delete back, then a hard break
    const steps = ["S", "SE", "SEC", "SECR", "SEC", "", "Hello", "Hello\n", "Hello\nWorld"];
    for (const text of steps) {
      const p = await doc.layoutEdit(0, rects, text);
      expect(p).toMatchObject({ docId: doc.id, revision: 0, fits: true });
      expect(p.lines.map((l) => l.text)).toEqual(text === "" ? [] : text.split("\n"));
      const r = await committed(text);
      expect(r.accepted).toBe(true);
      expect(r.layouts[0]).toMatchObject({ lines: p.lines.length, approximate: p.approximate, runs: p.runs, fingerprint: p.fingerprint });
      expect(seen.get(text) ?? p.fingerprint).toBe(p.fingerprint); // a repeated text ("SEC") repeats its fingerprint
      seen.set(text, p.fingerprint);
    }
    expect(new Set(seen.values()).size).toBe(seen.size); // distinct texts never share one
    expect(doc.revision).toBe(0);
    expect((await doc.extractText(0)).text).toBe(before);
    // Same inputs, same fingerprint; glyph boxes only on request.
    const a = await doc.layoutEdit(0, rects, "Hello\nWorld");
    const b = await doc.layoutEdit(0, rects, "Hello\nWorld", { glyphs: true });
    expect(b.fingerprint).toBe(a.fingerprint);
    expect(a.glyphs).toBeUndefined();
    expect(b.glyphs!.length).toBe(10);
    expect(b.lines[1].baseline).toBeGreaterThan(b.lines[0].baseline);
    expect(isCurrentPreview(b, doc)).toBe(true);
  });

  it("composes diacritics: decomposed input previews as its NFC form with the same fingerprint", async () => {
    const { doc, rects } = await open(true);
    const composed = await doc.layoutEdit(0, rects, "Žaliasis rašalas");
    const decomposed = await doc.layoutEdit(0, rects, "Žaliasis rašalas");
    expect(decomposed.fingerprint).toBe(composed.fingerprint);
    expect(decomposed.lines[0].text).toBe("Žaliasis rašalas");
    expect(decomposed.runs).toEqual(composed.runs);
  });

  it("falls back to a registered font for characters the source font lacks, and refuses without one", async () => {
    const { doc, rects } = await open(true);
    const p = await doc.layoutEdit(0, rects, "ąčęėįšųūž");
    expect(p.approximate).toBe(true);
    expect(p.runs.map((r) => r.source)).toContain("registered");
    expect(p.runs.reduce((n, r) => n + r.clusters, 0)).toBe(9);
    // This fixture's source font cannot be reused, so everything is the registered font (the Rust suite covers
    // source-font runs): the preview must say so, as the commit does.
    expect((await committed("ąčęėįšųūž")).layouts[0]).toMatchObject({ approximate: true, fingerprint: p.fingerprint, runs: p.runs });

    const bare = await open(false);
    await expect(bare.doc.layoutEdit(0, bare.rects, "ąč")).rejects.toMatchObject({ code: "EDIT_NO_FONT" });
    // After a font is registered the same call succeeds, with a new registry version.
    await bare.f.registerFonts([{ bytes: sans.slice(), family: "Liberation Sans", weight: 400, style: "normal" }]);
    const after = await bare.doc.layoutEdit(0, bare.rects, "ąč");
    expect(after.fontRegistryVersion).toBeGreaterThan(0);
  });

  it("reports overflow as fits false with the counts its commit refuses with", async () => {
    const { doc, rects } = await open(true);
    const text = "word ".repeat(500);
    const p = await doc.layoutEdit(0, rects, text);
    expect(p.fits).toBe(false);
    expect(p.neededLines).toBeGreaterThan(p.maxLines);
    expect(p.lines.length).toBe(p.neededLines);
    const r = await doc.applyChanges({ baseRevision: 0, textEdits: [{ id: "big", pageIndex: 0, rects, text }] });
    expect(r.rejected[0]).toMatchObject({ code: "EDIT_NO_FIT", details: { neededLines: p.neededLines, maxLines: p.maxLines } });
  });

  it("is stale after a commit, and validates its arguments", async () => {
    const { doc, rects } = await open(true);
    const p = await doc.layoutEdit(0, rects, "one");
    await doc.applyChanges({ baseRevision: 0, textEdits: [{ id: "e", pageIndex: 0, rects, text: "one" }] });
    expect(isCurrentPreview(p, doc)).toBe(false);
    expect((await doc.layoutEdit(0, [{ x: 0, y: 0, width: 1, height: 1 }], "x").catch((e) => e)).code).toBe("EDIT_NO_STYLE");
    await expect(doc.layoutEdit(-1, rects, "x")).rejects.toMatchObject({ code: "INVALID_ARGUMENT" });
    await expect(doc.layoutEdit(0, rects, 5 as unknown as string)).rejects.toBeInstanceOf(FoliarkError);
    await doc.close();
    await expect(doc.layoutEdit(0, rects, "x")).rejects.toMatchObject({ code: "DOCUMENT_GONE" });
  });
});
