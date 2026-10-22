// The public API end to end: client -> validated protocol -> worker core -> the real wasm engine.
// The worker runs in-process over a real MessageChannel, so buffers really transfer (detach) like in a browser.
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import init from "../wasm/foliark_wasm.js";
import { createFoliark, FoliarkError, type DocumentHandle, type Foliark } from "../src/index.js";
import { createWorkerCore } from "../src/worker-core.js";

const wasmBytes = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const fixture = (path: string) => new Uint8Array(readFileSync(new URL(`../../../tests/fixtures/${path}`, import.meta.url)));
const font = new Uint8Array(readFileSync(new URL("../../../crates/foliark-text/tests/data/LiberationSans-Regular.ttf", import.meta.url)));

let started = 0;
/** `stepBudget` 1 makes every page its own step, so a task spans many event-loop turns like a long page does. */
const worker = (stepBudget?: number) => (): Worker => {
  const { port1, port2 } = new MessageChannel();
  const core = createWorkerCore({
    post: (m, transfer = []) => port2.postMessage(m, { transfer }),
    loadWasm: () => init({ module_or_path: wasmBytes }).then(() => undefined),
  }, stepBudget);
  port2.onmessage = (e) => core.onMessage(e.data);
  started++;
  return Object.assign(port1, { terminate: () => (port1.close(), port2.close()) }) as unknown as Worker;
};
const clients: Foliark[] = [];
const client = async (o: Partial<Parameters<typeof createFoliark>[0]> = {}) => {
  const f = await createFoliark({ wasmUrl: "unused", worker: worker(), ...o });
  clients.push(f);
  return f;
};
afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.dispose()));
});

const rejects = async (p: Promise<unknown>) => {
  try {
    await p;
  } catch (e) {
    expect(e).toBeInstanceOf(FoliarkError);
    return e as FoliarkError;
  }
  throw new Error("expected a rejection");
};
const open = async (path = "hddn/rotation-crop.pdf", f?: Foliark): Promise<DocumentHandle> =>
  (f ?? (await client())).open(fixture(path));

describe("a document end to end", () => {
  it("opens with preflight info, geometry and one revision-0 handle", async () => {
    const f = await client();
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    const [info, pages] = [await doc.info(), await doc.pages()];
    expect([doc.generation, f.generation, doc.revision, info.pageCount, pages.length]).toEqual([1, 1, 0, 4, 4]);
    expect(pages.map((p) => p.rotation)).toContain(90);
    expect(await doc.pages()).toBe(pages); // geometry is cached, it cannot change between revisions
  });

  it("extracts text whose offsets slice the text, with astral characters counted in UTF-16", async () => {
    const doc = await open("hddn/unicode-glyph-widths.pdf");
    const page = await doc.extractText(0, { chars: true });
    expect(page).toMatchObject({ docId: doc.id, revision: 0, pageIndex: 0 });
    expect(page.text).toContain("😀");
    for (const w of page.words) expect(page.text.slice(w.start, w.end)).toBe(w.text);
    for (const c of page.clusters!) expect(c.end).toBeGreaterThan(c.start);
    expect((await doc.extractText(0)).clusters).toBeUndefined();
  });

  it("searches every page, hits carry page and revision, and quadsFor/hitTest/select agree with them", async () => {
    const doc = await open("hddn/shared-vector.pdf");
    const hits = await doc.search("shared_vector");
    const pageIndexes = hits.map((h) => h.pageIndex);
    expect([...new Set(pageIndexes)]).toEqual([0, 1]);
    expect(pageIndexes).toEqual([...pageIndexes].sort()); // page order
    expect(hits.every((h) => h.revision === 0 && h.text.toLowerCase().includes("shared_vector"))).toBe(true);
    const [first] = hits;
    const text = await doc.extractText(first.pageIndex);
    expect(text.text.slice(first.start, first.end)).toBe(first.text);
    const again = await doc.quadsFor(first.pageIndex, first.start, first.end);
    expect(again.quads).toEqual(first.quads);
    const q = first.quads[0];
    const inside = { x: (q[0].x + q[2].x) / 2, y: (q[0].y + q[2].y) / 2 };
    const hit = await doc.hitTest(first.pageIndex, inside);
    expect(hit && hit.start >= first.start && hit.end <= first.end).toBe(true);
    const sel = await doc.select(first.pageIndex, q[0], q[2]);
    expect(sel?.text.length).toBeGreaterThan(0);
    expect(await doc.hitTest(first.pageIndex, { x: 1e6, y: 1e6 })).toBeNull();
    expect(await doc.search("no such text anywhere")).toEqual([]);
  });

  it("renders RGBA pixels of exactly width*height*4 bytes and rejects out-of-range pages", async () => {
    const doc = await open("hddn/rotation-crop.pdf");
    const [g] = await doc.pages();
    const r = await doc.renderPage(0, { scale: 0.5 });
    expect([r.width, r.height]).toEqual([Math.max(1, Math.round(g.width / 2)), Math.max(1, Math.round(g.height / 2))]);
    expect(r.data.length).toBe(r.width * r.height * 4);
    expect(r.data.some((v) => v !== 255)).toBe(true); // something was drawn
    expect((await rejects(doc.renderPage(9))).code).toBe("INVALID_ARGUMENT");
    expect((await rejects(doc.extractText(-1))).code).toBe("INVALID_ARGUMENT");
    expect((await rejects(doc.hitTest(0, { x: NaN, y: 0 }))).code).toBe("INVALID_ARGUMENT");
  });

  it("keeps documents independent in one worker and honours maxOpenDocuments", async () => {
    const f = await client({ limits: { maxOpenDocuments: 2 } });
    const [a, b] = [await f.open(fixture("hddn/rotation-crop.pdf")), await f.open(fixture("hddn/shared-image.pdf"))];
    expect((await rejects(f.open(fixture("hddn/acroform.pdf")))).code).toBe("LIMIT_EXCEEDED");
    await a.close();
    expect((await b.extractText(0)).docId).toBe(b.id);
    await f.open(fixture("hddn/acroform.pdf")); // the slot freed on close
  });

  it("surfaces typed open failures", async () => {
    const f = await client();
    expect((await rejects(f.open(fixture("hddn/encrypted.pdf")))).code).toBe("PASSWORD_REQUIRED");
    expect((await rejects(f.open(fixture("hddn/encrypted.pdf"), { password: "wrong" }))).code).toBe("PASSWORD_INCORRECT");
    const doc = await f.open(fixture("hddn/encrypted.pdf"), { password: "hddn-fixture-user" });
    expect((await doc.info()).encrypted).toBe(true);
    expect((await rejects(f.open(new Uint8Array(100)))).code).toBe("MALFORMED_PDF");
  });
});

describe("handles, generations and buffers", () => {
  it("rejects every call on a closed handle with DOCUMENT_GONE, and close is idempotent", async () => {
    const doc = await open();
    await doc.close();
    await doc.close();
    for (const call of [() => doc.pages(), () => doc.extractText(0), () => doc.search("x"), () => doc.renderPage(0)]) {
      const e = await rejects(call());
      expect([e.code, e.docId]).toEqual(["DOCUMENT_GONE", doc.id]);
    }
  });

  it("restart kills old handles, rejects in-flight calls WORKER_RESET, re-sends fonts and fires onReset", async () => {
    const f = await client();
    const resets: unknown[] = [];
    f.onReset((e) => resets.push(e));
    await f.registerFonts([{ bytes: font, family: "Liberation Sans", weight: 400, style: "normal" }]);
    const old = await f.open(fixture("hddn/rotation-crop.pdf"));
    const inflight = old.renderPage(0, { scale: 4 });
    const restarted = f.restart();
    const e = await rejects(inflight);
    expect([e.code, e.recoverable]).toEqual(["WORKER_RESET", true]);
    await restarted;
    expect([f.generation, resets]).toEqual([2, [{ generation: 2, reason: "restart" }]]);
    expect((await rejects(old.pages())).code).toBe("DOCUMENT_GONE");
    expect(started).toBeGreaterThanOrEqual(2);
    const fresh = await f.open(fixture("hddn/rotation-crop.pdf"));
    expect(fresh.generation).toBe(2);
    // The font survived the reset: registering the same face again replaces it instead of failing.
    expect((await f.registerFonts([{ bytes: font, family: "Liberation Sans", weight: 400, style: "normal" }])).length).toBe(1);
  });

  it("detaches a transferred input buffer, copies views, and keeps the buffer with transfer:false", async () => {
    const f = await client();
    const bytes = fixture("hddn/rotation-crop.pdf");
    const buf = bytes.slice().buffer;
    await f.open(buf);
    expect(buf.byteLength).toBe(0);
    const kept = bytes.slice().buffer;
    await f.open(kept, { transfer: false });
    expect(kept.byteLength).toBe(bytes.length);
    const view = bytes.slice();
    await f.open(view);
    expect(view.byteLength).toBe(bytes.length);
  });

  it("registers fonts atomically and reports a bad one as FONT_UNSUPPORTED", async () => {
    const f = await client();
    const good = { bytes: font, family: "Liberation Sans", weight: 400 as const, style: "normal" as const };
    const reg = await f.registerFonts([good]);
    expect(reg[0]).toMatchObject({ family: "Liberation Sans", weight: 400, style: "normal" });
    expect(reg[0].glyphCount).toBeGreaterThan(100);
    const e = await rejects(f.registerFonts([{ ...good, family: "Other" }, { ...good, bytes: new Uint8Array(64) }]));
    expect(e.code).toBe("FONT_UNSUPPORTED");
  });

  it("dispose rejects later calls with CLIENT_DISPOSED, handles included", async () => {
    const f = await client();
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    await f.dispose();
    expect((await rejects(f.open(fixture("hddn/rotation-crop.pdf")))).code).toBe("CLIENT_DISPOSED");
    expect((await rejects(doc.pages())).code).toBe("CLIENT_DISPOSED");
  });
});

describe("cancellation", () => {
  const many = async () => {
    const f = await client({ worker: worker(1), limits: { maxPages: 2000 } });
    return f.open(fixture("generated/many-pages-1001.pdf"));
  };

  it("cancels an already aborted call before it is sent", async () => {
    const doc = await open();
    expect((await rejects(doc.extractText(0, { signal: AbortSignal.abort() }))).code).toBe("CANCELLED");
    expect((await doc.extractText(0)).pageIndex).toBe(0);
  });

  it("cancels a queued call before it starts and a running search between steps, leaving the document usable", async () => {
    const doc = await many();
    const [queuedAbort, runningAbort] = [new AbortController(), new AbortController()];
    const running = doc.search("PAGE_COMMON_TEXT", { signal: runningAbort.signal });
    const queued = doc.extractText(0, { signal: queuedAbort.signal });
    queuedAbort.abort();
    setTimeout(() => runningAbort.abort(), 0);
    const settled = await Promise.allSettled([queued, running]);
    expect(settled.map((r) => r.status === "rejected" && r.reason.code)).toEqual(["CANCELLED", "CANCELLED"]);
    expect((await doc.search("PAGE_COMMON_TEXT", { pages: [0, 1] })).length).toBe(2);
  });

  it("stepping does not change the result", async () => {
    const stepped = await (await many()).search("PAGE_COMMON_TEXT", { maxHits: 50 });
    const whole = await (await open("generated/many-pages-1001.pdf", await client({ limits: { maxPages: 2000 } }))).search("PAGE_COMMON_TEXT", { maxHits: 50 });
    expect(stepped.length).toBe(50);
    expect(stepped).toEqual(whole);
  });
});

describe("applyChanges", () => {
  it("commits a redaction list atomically, moves the revision, and refuses a stale or partly bad ChangeSet", async () => {
    const doc = await open("hddn/unicode-glyph-widths.pdf");
    const word = (await doc.extractText(0)).words[0];
    const region = { kind: "region", id: "r", pageIndex: 0, rect: { x: 0, y: 0, width: 20, height: 20 } } as const;
    const cs = {
      baseRevision: 0,
      redactions: [region, { kind: "text", id: "t", pageIndex: 0, start: word.start, end: word.end, fill: [1, 0, 0] }],
    } as const;
    const r = await doc.applyChanges(cs as never);
    expect(r).toMatchObject({ accepted: true, revision: 1, rejected: [], changedPages: [0], layouts: [] });
    expect([doc.revision, (await doc.extractText(0)).revision]).toEqual([1, 1]);

    // The same request again is a stale base: no second commit, the handle keeps revision 1.
    const stale = await rejects(doc.applyChanges(cs as never));
    expect([stale.code, doc.revision]).toEqual(["STALE_REVISION", 1]);

    // One bad item rejects the set: nothing moves and the rejection names the item.
    const bad = { ...region, id: "x", pageIndex: 99 };
    const refused = await doc.applyChanges({ baseRevision: 1, redactions: [region, bad] });
    expect(refused).toMatchObject({ accepted: false, revision: 1, rejected: [{ id: "x", code: "INVALID_ARGUMENT" }] });
    expect(doc.revision).toBe(1);
  });
});

describe("ops that are not built yet", () => {
  it("refuse with a typed error naming the feature, and leave the document usable", async () => {
    const doc = await open();
    const refused = async (p: Promise<unknown>, feature: string) => {
      const e = await rejects(p);
      expect([e.code, e.details?.feature]).toEqual(["INVALID_ARGUMENT", feature]);
    };
    await refused(doc.prepareParagraph(0, [{ x: 0, y: 0, width: 10, height: 10 }]), "prepareParagraph");
    expect(doc.revision).toBe(0);
    expect((await doc.extractText(0)).pageIndex).toBe(0);
  });
});
