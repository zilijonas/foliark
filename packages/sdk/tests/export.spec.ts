// Native export through the SDK (spec 4, 5, 8): open -> applyChanges(text redaction) -> prepareExport -> export,
// then the exported bytes are opened by the SDK again. Plans are single use and die with their revision.
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import init from "../wasm/foliark_wasm.js";
import { createFoliark, FoliarkError, type DocumentHandle, type Foliark } from "../src/index.js";
import { createWorkerCore } from "../src/worker-core.js";

const wasmBytes = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const fixture = (name: string) => new Uint8Array(readFileSync(new URL(`../../../tests/fixtures/${name}`, import.meta.url)));

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
const client = async (): Promise<Foliark> => {
  const f = await createFoliark({ wasmUrl: "unused", worker });
  clients.push(f);
  return f;
};
afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.dispose()));
});
const code = async (p: Promise<unknown>) => {
  try {
    await p;
  } catch (e) {
    expect(e).toBeInstanceOf(FoliarkError);
    return (e as FoliarkError).code;
  }
  throw new Error("expected a rejection");
};

const SECRET = "SECRET_METADATA_XMP";
const KEEP = "KEEP_PUBLIC_METADATA_XMP";

async function redacted(f: Foliark): Promise<DocumentHandle> {
  const doc = await f.open(fixture("hddn/metadata-xmp.pdf"));
  const start = (await doc.extractText(0)).text.indexOf(SECRET);
  expect(start).toBeGreaterThanOrEqual(0);
  const r = await doc.applyChanges({ baseRevision: 0, redactions: [{ kind: "text", id: "r", pageIndex: 0, start, end: start + SECRET.length }] });
  expect(r.accepted).toBe(true);
  return doc;
}

describe("text redaction and sanitized native export", () => {
  it("the exported file reopens with the word gone, the rest intact and the metadata stripped", async () => {
    const f = await client();
    const doc = await redacted(f);
    const plan = await doc.prepareExport({ mode: "native" });
    expect([plan.revision, plan.mode, plan.docId]).toEqual([1, "native", doc.id]);
    expect(plan.report.redaction.clustersRemoved).toBe(SECRET.length);
    expect(plan.report.removed.metadata).toBe(2); // Info + XMP
    const bytes = await doc.export(plan);
    expect(bytes.byteLength).toBe(plan.byteLength);

    const out = await f.open(bytes);
    const text = (await out.extractText(0)).text;
    expect(text).not.toContain(SECRET);
    expect(text).toContain(KEEP);
    expect((await out.info()).features.metadata).toBe(false);
    // The raw bytes (streams are deflated, so this only catches strings stored plain) hold no secret either.
    expect(Buffer.from(bytes).includes(SECRET)).toBe(false);
  });

  it("a plan exports once and dies with its revision; capabilities follow what prepareExport proved", async () => {
    const f = await client();
    const doc = await redacted(f);
    const plan = await doc.prepareExport({ mode: "native" });
    expect((await doc.capabilities()).exportNative.status).toBe("supported");
    await doc.export(plan);
    expect(await code(doc.export(plan))).toBe("PLAN_CONSUMED");

    const again = await doc.prepareExport({ mode: "native" });
    await doc.applyChanges({ baseRevision: 1, redactions: [] });
    expect(await code(doc.export(again))).toBe("STALE_PLAN");
    expect((await doc.capabilities()).exportNative.status).toBe("unknown");
  });

  it("an unsupported stage refuses with a typed code and the document stays usable", async () => {
    const f = await client();
    const doc = await f.open(fixture("hddn/acroform.pdf"));
    const caps = await doc.capabilities();
    expect([caps.exportNative.status, caps.exportRaster.status]).toEqual(["unsupported", "unknown"]);
    try {
      await doc.prepareExport({ mode: "native" });
      throw new Error("expected a refusal");
    } catch (e) {
      expect([(e as FoliarkError).code, (e as FoliarkError).details?.feature]).toEqual(["INVALID_ARGUMENT", "flattenWidgets"]);
    }
    expect(await code(doc.prepareExport({ mode: "raster" }))).toBe("CONSENT_REQUIRED");
    expect((await doc.extractText(0)).pageIndex).toBe(0);
  });

  it("a raster export succeeds where native refuses, keeps the page geometry and has no text layer", async () => {
    const f = await client();
    const doc = await f.open(fixture("hddn/acroform.pdf"));
    expect(await code(doc.prepareExport({ mode: "native" }))).toBe("INVALID_ARGUMENT");
    const plan = await doc.prepareExport({ mode: "raster", policy: { raster: { acknowledgeLosses: true, scale: 1 } } });
    expect(plan.mode).toBe("raster");
    const lost = plan.report.losses.map((l) => l.code);
    expect(lost).toEqual(expect.arrayContaining(["TEXT_SEARCH_LOST", "VECTOR_RASTERIZED", "FORMS_FLATTENED"]));
    expect((await doc.capabilities()).exportRaster.status).toBe("supported");
    const bytes = await doc.export(plan);
    expect(bytes.byteLength).toBe(plan.report.outputBytes);

    const out = await f.open(bytes);
    expect((await out.extractText(0)).text).toBe("");
    const [src, dst] = [(await doc.pages())[0], (await out.pages())[0]];
    expect([dst.width, dst.height, dst.rotation]).toEqual([src.width, src.height, 0]);
    const page = await out.renderPage(0, { scale: 1 });
    expect(page.complete).toBe(true);
  });
});
