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
});

/** A one-page 300 x 400 PDF from object bodies 3.. (1 catalog and 2 page tree are added). */
function pdf(page: string, objs: string[]): Uint8Array {
  const bodies = ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", page, ...objs];
  let out = "%PDF-1.7\n";
  const at = bodies.map((b, i) => {
    const o = out.length;
    out += `${i + 1} 0 obj\n${b}\nendobj\n`;
    return o;
  });
  const x = out.length;
  out += `xref\n0 ${bodies.length + 1}\n0000000000 65535 f \n${at.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("")}`;
  out += `trailer\n<< /Size ${bodies.length + 1} /Root 1 0 R >>\nstartxref\n${x}\n%%EOF\n`;
  return new TextEncoder().encode(out);
}
const stream = (dict: string, data: string) => `<< ${dict} /Length ${data.length} >>\nstream\n${data}\nendstream`;
const region = (id: string, y: number, height: number) =>
  ({ kind: "region", id, pageIndex: 0, rect: { x: 0, y, width: 300, height } }) as const;

describe("image and vector redaction over the real wasm", () => {
  const page = (res: string) => `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << ${res} >> >>`;

  it("exports with the image erased and the vector removed, and reports what the stages did", async () => {
    const f = await client();
    const content = "q 60 0 0 60 20 200 cm /Im Do Q\n1 0 0 rg 120 200 60 60 re f\n";
    const image = stream(
      "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /ASCIIHexDecode",
      "FF0000 00FF00 0000FF FFFF00>",
    );
    const doc = await f.open(pdf(page("/XObject << /Im 5 0 R >>"), [stream("", content), image]));
    // Display space is y-down: both objects sit at display y 140..200.
    expect((await doc.applyChanges({ baseRevision: 0, redactions: [region("all", 100, 120)] })).accepted).toBe(true);
    const plan = await doc.prepareExport({ mode: "native" });
    expect(plan.report.redaction).toEqual({
      regions: 1, clustersRemoved: 0, imagesErased: 1, imagesCloned: 0, formsCloned: 0,
      vectorSubpathsRemoved: 1, vectorRectsCut: 0, collateral: [], sharedSurvivors: [],
    });
    expect(plan.report.losses).toContainEqual({ code: "IMAGE_REENCODED", count: 1, pages: [0] });
    expect((await (await f.open(await doc.export(plan))).info()).pageCount).toBe(1);
  });

  it("a refusal keeps its code, feature, pages and redaction ids across the wire", async () => {
    const f = await client();
    const pattern = stream(
      "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>",
      "0 0 5 5 re f",
    );
    const bytes = pdf(page("/Pattern << /P1 5 0 R >>"), [stream("", "/Pattern cs /P1 scn 200 100 80 80 re f\n"), pattern]);
    const doc = await f.open(bytes);
    await doc.applyChanges({ baseRevision: 0, redactions: [region("pat", 200, 120)] });
    const e = await doc.prepareExport({ mode: "native" }).then(
      () => undefined,
      (err: unknown) => err as FoliarkError,
    );
    expect([e?.code, e?.details]).toEqual(["REDACT_PATTERN_UNSUPPORTED", { feature: "redactionPatterns", pages: [0], ids: ["pat"] }]);
  });
});
