// Edit, export, open the export and edit again over the real wasm, as HDDN does (0.3.2): prepareParagraph with a
// registered font, applyChanges, export, then the same on the exported file, three times. The Rust suite
// (crates/foliark-engine/tests/edit_round_trip.rs) covers both font paths, space glyphs and the fidelity oracle; this
// proves the protocol path on a pdf-lib shaped file, judged by PyMuPDF.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, afterEach, expect, it } from "vitest";
import init from "../wasm/foliark_wasm.js";
import { createFoliark, type Foliark } from "../src/index.js";
import { createWorkerCore } from "../src/worker-core.js";

const root = new URL("../../../", import.meta.url).pathname;
const wasmBytes = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const pdf = new Uint8Array(readFileSync(join(root, "tests/fixtures/generated/pdflib-type0-subset.pdf")));
const sans = new Uint8Array(readFileSync(join(root, "crates/foliark-text/tests/data/LiberationSans-Regular.ttf")));
const python = join(root, "tests/oracles/.venv/bin/python");
const scratch = mkdtempSync(join(tmpdir(), "foliark-edit-round-trip-"));
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

const fitzText = (name: string, bytes: Uint8Array) => {
  const file = join(scratch, name);
  writeFileSync(file, bytes);
  const code = "import sys, fitz\nprint(''.join(p.get_text() for p in fitz.open(sys.argv[1])))\n";
  return execFileSync(python, ["-c", code, file], { encoding: "utf8" }).replace(/\s+/g, "");
};

it("a registered-font edit of an exported file reads back exactly, three rounds", async () => {
  const lecture = { x: 15, y: 89, width: 270, height: 14 };
  let bytes = pdf;
  for (const [round, text] of ["Lecture for the third-age xyz", "Lecture for the xyz college", "Lecture xyz on the zz college"].entries()) {
    const f = await createFoliark({ wasmUrl: "unused", worker });
    clients.push(f);
    const [font] = await f.registerFonts([{ bytes: sans.slice(), family: "Liberation Sans", weight: 400, style: "normal" }]);
    const doc = await f.open(bytes.slice());
    await doc.prepareParagraph(0, [lecture], { font: font.id });
    const r = await doc.applyChanges({ baseRevision: 0, textEdits: [{ id: `e${round}`, pageIndex: 0, rects: [lecture], text, font: font.id }] });
    expect(r.rejected).toEqual([]);
    bytes = await doc.export(await doc.prepareExport({ mode: "native" }));
    const read = fitzText(`round${round}.pdf`, bytes);
    expect(read).toContain(text.replace(/\s+/g, ""));
    expect(read).toContain("KEEP_PUBLIC_PDFLIB");
    expect(read).not.toContain("�");
  }
});
