// NEW-88: the CMap pack is fetched at open, base first, before the first page is read (spec 10.3). Real wasm; the pack
// folder is served by an in-process fetch so the request log is exact.
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import init from "../wasm/foliark_wasm.js";
import { createFoliark, FoliarkError, type Foliark } from "../src/index.js";
import { createWorkerCore } from "../src/worker-core.js";

const wasmBytes = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const fixture = (path: string) => new Uint8Array(readFileSync(new URL(`../../../tests/fixtures/${path}`, import.meta.url)));
const pack = (name: string) => new Uint8Array(readFileSync(new URL(`../cmaps/${name}.cmap`, import.meta.url)));
const CJK = "generated/cjk-predefined-cmaps.pdf";
const BASE = "https://app.test/assets/cmaps/";

const requests: string[] = [];
let serve: (name: string) => Uint8Array | undefined = pack;
const clients: Foliark[] = [];
const client = async (cmapsUrl?: string) => {
  const f = await createFoliark({
    wasmUrl: "unused",
    cmapsUrl,
    worker: () => {
      const { port1, port2 } = new MessageChannel();
      const core = createWorkerCore({
        post: (m, transfer = []) => port2.postMessage(m, { transfer }),
        loadWasm: () => init({ module_or_path: wasmBytes }).then(() => undefined),
        cmapsUrl: BASE,
        fetchCMap: async (url) => {
          requests.push(url);
          const bytes = serve(url.slice(url.lastIndexOf("/") + 1).replace(/\.cmap$/, ""));
          if (!bytes) throw new Error("HTTP 404");
          return bytes;
        },
      });
      port2.onmessage = (e) => core.onMessage(e.data);
      return Object.assign(port1, { terminate: () => (port1.close(), port2.close()) }) as unknown as Worker;
    },
  });
  clients.push(f);
  return f;
};
afterEach(async () => {
  requests.length = 0;
  serve = pack;
  await Promise.all(clients.splice(0).map((c) => c.dispose()));
});
const rejected = async (p: Promise<unknown>) => {
  try {
    await p;
  } catch (e) {
    expect(e).toBeInstanceOf(FoliarkError);
    return e as FoliarkError;
  }
  throw new Error("expected a rejection");
};

describe("predefined CMaps", () => {
  it("loads exactly the needed pack files once, then extracts and renders the CJK text", async () => {
    const f = await client();
    const doc = await f.open(fixture(CJK));
    expect((await doc.info()).requiredAssets).toEqual(["cmap:Adobe-GB1-UCS2", "cmap:UniGB-UCS2-H", "cmap:UniJIS-UCS2-H"]);
    expect(requests).toEqual(["Adobe-GB1-UCS2", "UniGB-UCS2-H", "UniJIS-UCS2-H"].map((n) => `${BASE}${n}.cmap`));
    const page = await doc.extractText(0);
    expect(page.text.split(/\s+/)).toEqual(["KEEP_PUBLIC_CJK", "中文测试", "日本語", "你好"]);
    const img = await doc.renderPage(0, { scale: 1 });
    expect(img.warnings.map((w) => w.code)).not.toContain("CMAP_MISSING");
    await f.open(fixture(CJK)); // a second document finds the maps registered
    expect(requests).toHaveLength(3);
  });

  it("a document with no predefined CMap fetches nothing", async () => {
    await (await client()).open(fixture("hddn/rotation-crop.pdf"));
    expect(requests).toEqual([]);
  });

  it("a cmapsUrl option replaces the default location", async () => {
    await (await client("https://cdn.test/own/")).open(fixture(CJK));
    expect(requests[0]).toBe("https://cdn.test/own/Adobe-GB1-UCS2.cmap");
  });

  it("a missing pack file rejects open with ASSET_LOAD_FAILED naming it, and leaves no document open", async () => {
    serve = (n) => (n === "UniJIS-UCS2-H" ? undefined : pack(n));
    const f = await client();
    for (let i = 0; i < 3; i++) { // maxOpenDocuments is 2: a leaked handle would turn the third into LIMIT_EXCEEDED
      const e = await rejected(f.open(fixture(CJK)));
      expect([e.code, e.details?.feature]).toEqual(["ASSET_LOAD_FAILED", "cmap:UniJIS-UCS2-H"]);
      expect(e.message).toMatch(/UniJIS-UCS2-H.*404.*cmapsUrl/);
    }
    serve = pack;
    await f.open(fixture(CJK)); // the failure is not sticky
  });

  it.each([
    ["truncated", (b: Uint8Array) => b.slice(0, b.length >> 1)],
    ["another map's bytes", () => pack("UniGB-UCS2-H")],
    ["not a CMap at all", () => new TextEncoder().encode("<html>404 page</html>")],
  ])("a corrupt pack file (%s) rejects open with ASSET_LOAD_FAILED", async (_, damage) => {
    serve = (n) => (n === "UniJIS-UCS2-H" ? damage(pack(n)) : pack(n));
    const e = await rejected((await client()).open(fixture(CJK)));
    expect([e.code, e.details?.feature]).toEqual(["ASSET_LOAD_FAILED", "cmap:UniJIS-UCS2-H"]);
    expect(e.message).toMatch(/corrupt or incomplete/);
  });

  it("a map that usecmaps another is registered after its base", async () => {
    // UniGB-UCS2-V says `/UniGB-UCS2-H usecmap`; the same-length rename makes the document name only the V map
    const seen: string[] = [];
    serve = (n) => (seen.push(n), pack(n));
    const pdf = new Uint8Array(Buffer.from(Buffer.from(fixture(CJK)).toString("latin1").replaceAll("/UniGB-UCS2-H", "/UniGB-UCS2-V"), "latin1"));
    const doc = await (await client()).open(pdf);
    expect((await doc.info()).requiredAssets).toContain("cmap:UniGB-UCS2-V");
    expect(seen).toEqual(expect.arrayContaining(["UniGB-UCS2-V", "UniGB-UCS2-H"])); // open succeeding is the order proof
  });
});
