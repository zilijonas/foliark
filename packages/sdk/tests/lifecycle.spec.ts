// Deterministic release (spec 1, 10.2): close frees the document and its caches, a reset or dispose leaves no
// reachable handle, pending work always settles, and a long open/use/close loop does not grow. The allocation
// counters are the wasm engine's own `stats()` (live documents, cache bytes, live tasks, linear memory); nothing
// here claims physical memory is erased.
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import init, * as wasm from "../wasm/foliark_wasm.js";
import { createFoliark, FoliarkError, type Foliark } from "../src/index.js";
import { createWorkerCore } from "../src/worker-core.js";

const wasmBytes = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const fixture = (path: string) => new Uint8Array(readFileSync(new URL(`../../../tests/fixtures/${path}`, import.meta.url)));
const font = new Uint8Array(readFileSync(new URL("../../../crates/foliark-text/tests/data/LiberationSans-Regular.ttf", import.meta.url)));
const pdf = fixture("hddn/rotation-crop.pdf");

const worker = (stepBudget?: number) => (): Worker => {
  const { port1, port2 } = new MessageChannel();
  const core = createWorkerCore({
    post: (m, transfer = []) => port2.postMessage(m, { transfer }),
    loadWasm: () => init({ module_or_path: wasmBytes }).then(() => undefined),
  }, stepBudget);
  port2.onmessage = (e) => core.onMessage(e.data);
  return Object.assign(port1, { terminate: () => (port1.close(), port2.close()) }) as unknown as Worker;
};
const clients: Foliark[] = [];
const client = async (stepBudget?: number) => {
  const f = await createFoliark({ wasmUrl: "unused", worker: worker(stepBudget) });
  clients.push(f);
  return f;
};
afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.dispose()));
});
const settle = (p: Promise<unknown>) => p.then((v) => v, (e) => e);

/** The engine without the worker around it, to read its counters. */
async function engine() {
  await init({ module_or_path: wasmBytes });
  return new wasm.Engine(undefined, 1);
}
const stats = (e: wasm.Engine) => e.stats() as Record<"documents" | "sourceBytes" | "cacheEntries" | "cacheBytes" | "cacheEvictions" | "tasks" | "memoryBytes", number>;
const drive = (e: wasm.Engine, task: number): any => {
  for (;;) {
    const s = e.step(task, 1_000_000);
    if (s.done) return s.result;
  }
};

describe("engine counters", () => {
  it("open, extract, search, render, hit-test and close loops return every counter to zero and linear memory stops growing", async () => {
    const e = await engine();
    const once = () => {
      const { docId } = e.open(pdf.slice(), undefined, undefined);
      for (let p = 0; p < 4; p++) drive(e, e.startExtractText(docId, p, {}));
      drive(e, e.startSearch(docId, "ROTATION", {}));
      drive(e, e.startRender(docId, 0, { scale: 1 }));
      e.hitTest(docId, 0, { x: 50, y: 50 }, undefined);
      const during = stats(e);
      e.close(docId);
      return during;
    };
    let during = once();
    expect(during).toMatchObject({ documents: 1, tasks: 0 });
    expect(during.cacheEntries).toBeGreaterThan(0);
    expect(during.cacheBytes).toBeGreaterThan(0);
    for (let i = 0; i < 5; i++) once(); // warm: the allocator reaches its working size
    const warm = stats(e).memoryBytes;
    for (let i = 0; i < 60; i++) {
      during = once();
      expect(stats(e)).toMatchObject({ documents: 0, sourceBytes: 0, cacheEntries: 0, cacheBytes: 0, tasks: 0 });
    }
    // 60 more cycles of a 1 MB-scale working set must not add a megabyte-scale footprint each.
    expect(stats(e).memoryBytes - warm).toBeLessThanOrEqual(2 << 20);
  });

  it("a task the host abandons or that fails leaves no entry in the task table", async () => {
    const e = await engine();
    const { docId } = e.open(pdf.slice(), undefined, undefined);
    const dropped = e.startSearch(docId, "x", {});
    e.cancel(dropped);
    e.cancel(dropped); // idempotent
    const failing = e.startExtractText(docId, 99, {}); // page out of range
    expect(() => e.step(failing, 1_000_000)).toThrow();
    const gone = e.startExtractText(docId, 0, {});
    e.close(docId);
    expect(() => e.step(gone, 1_000_000)).toThrow(); // DOCUMENT_GONE
    expect(stats(e)).toMatchObject({ documents: 0, tasks: 0, cacheEntries: 0 });
  });

  it("closing one document frees its caches and leaves the other's", async () => {
    const e = await engine();
    const [a, b] = [e.open(pdf.slice(), undefined, undefined).docId, e.open(pdf.slice(), undefined, undefined).docId];
    drive(e, e.startExtractText(a, 0, {}));
    drive(e, e.startExtractText(b, 0, {}));
    drive(e, e.startExtractText(b, 1, {}));
    expect(stats(e)).toMatchObject({ documents: 2, cacheEntries: 3 });
    e.close(a);
    e.close(a);
    expect(stats(e)).toMatchObject({ documents: 1, cacheEntries: 2 });
    e.close(b);
    expect(stats(e)).toMatchObject({ documents: 0, cacheEntries: 0, cacheBytes: 0, sourceBytes: 0 });
  });
});

describe("client disposal", () => {
  it("dispose during operations settles everything, kills the handles, and a new client works", async () => {
    const f = await client(1); // one page per step: ops span event-loop turns
    await f.registerFonts([{ bytes: font, family: "Liberation Sans", weight: 400, style: "normal" }]);
    const doc = await f.open(pdf.slice());
    const geometry = await doc.pages(); // cached on the handle
    const ctl = new AbortController();
    const calls = [
      doc.search("a"), doc.search("b", { signal: ctl.signal }), doc.renderPage(0), doc.extractText(1),
      f.open(pdf.slice()), f.registerFonts([{ bytes: font, family: "Other", weight: 400, style: "normal" }]),
    ];
    const disposing = f.dispose();
    const out = await Promise.all(calls.map(settle));
    await disposing;
    for (const e of out) {
      // Cut off by the dispose, or finished before it: never left hanging.
      if (e instanceof FoliarkError) expect(["CLIENT_DISPOSED", "CANCELLED"]).toContain(e.code);
    }
    expect(out.some((e) => e instanceof FoliarkError && e.code === "CLIENT_DISPOSED")).toBe(true);
    // Nothing is reachable through the old handle, not even what it had cached.
    expect(geometry.length).toBe(4);
    for (const call of [() => doc.pages(), () => doc.extractText(0), () => doc.close().then(() => doc.pages())]) {
      expect(((await settle(call())) as FoliarkError).code).toBe("CLIENT_DISPOSED");
    }
    for (const call of [() => f.open(pdf.slice()), () => f.registerFonts([]), () => f.restart()]) {
      const r = await settle(call());
      if (r instanceof Error) expect((r as FoliarkError).code).toBe("CLIENT_DISPOSED");
    }
    await f.dispose(); // idempotent
    // Reopen after dispose is a new client.
    const again = await client();
    expect(((await (await again.open(pdf.slice())).extractText(0)) as { revision: number }).revision).toBe(0);
  });

  it("a reset releases the old generation's handle caches, so geometry cached before it is not served after", async () => {
    const f = await client();
    const doc = await f.open(pdf.slice());
    await doc.pages();
    await f.restart();
    expect(((await settle(doc.pages())) as FoliarkError).code).toBe("DOCUMENT_GONE");
    await doc.close(); // idempotent and quiet on a dead handle
    const fresh = await f.open(pdf.slice());
    expect((await fresh.pages()).length).toBe(4);
  });

  it("close frees the slot at once and is idempotent under concurrent calls", async () => {
    const f = await createFoliark({ wasmUrl: "unused", worker: worker(), limits: { maxOpenDocuments: 1 } });
    clients.push(f);
    const doc = await f.open(pdf.slice());
    await Promise.all([doc.close(), doc.close(), doc.close()]);
    expect((await settle(doc.extractText(0)) as FoliarkError).code).toBe("DOCUMENT_GONE");
    expect((await (await f.open(pdf.slice())).pages()).length).toBe(4);
  });
});
