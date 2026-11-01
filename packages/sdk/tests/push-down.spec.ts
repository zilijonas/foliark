// Opt-in push-down over the real wasm (ADR 0015): `reflow: 'pushDown'` travels through `layoutEdit` and
// `applyChanges`, the preview reports the shift the commit makes, and `none` still refuses. The Rust suite
// (crates/foliark-engine/tests/push_down.rs) judges the moved output with the shift oracle.
import { readFileSync } from "node:fs";
import { afterEach, expect, it } from "vitest";
import init from "../wasm/foliark_wasm.js";
import { createFoliark, type Foliark } from "../src/index.js";
import { createWorkerCore } from "../src/worker-core.js";

const wasmBytes = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const pdf = new Uint8Array(readFileSync(new URL("../../../tests/fixtures/generated/push-letter.pdf", import.meta.url)));
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

const rects = [{ x: 60, y: 190, width: 480, height: 14 }];
const text =
  "Thank you for contacting us. We updated the details on your account:\nIt takes effect from the first of next month.\n" +
  "Please keep this letter for your records.";

it("previews and commits a push-down; without it the edit does not fit", async () => {
  const f = await createFoliark({ wasmUrl: "unused", worker });
  clients.push(f);
  await f.registerFonts([{ bytes: sans.slice(), family: "Liberation Sans", weight: 400, style: "normal" }]);
  const doc = await f.open(pdf.slice());
  const info = await doc.prepareParagraph(0, rects);
  expect(info.pushDown!.maxLines).toBeGreaterThan(info.maxWrapLines);
  const none = await doc.layoutEdit(0, rects, text);
  expect(none.fits).toBe(false);
  expect(none.shift ?? null).toBeNull();
  const p = await doc.layoutEdit(0, rects, text, { reflow: "pushDown" });
  expect(p.fits).toBe(true);
  expect(p.shift).toMatchObject({ pageIndex: 0, dy: 13.2 });
  const r = await doc.applyChanges({ baseRevision: 0, textEdits: [{ id: "e", pageIndex: 0, rects, text, reflow: "pushDown" }] });
  expect(r.accepted).toBe(true);
  expect(r.layouts[0].fingerprint).toBe(p.fingerprint);
  expect(r.remap).toEqual([p.shift]);
});
