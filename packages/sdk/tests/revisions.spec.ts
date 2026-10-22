// Revision binding (spec 4): every text result carries the revision it was computed at, a stale base or a task
// that straddles a commit is a typed STALE_REVISION, and cached text survives only commits that cannot change it.
// Text edits are not built yet (NEW-51), so commits here are redaction lists; edit commits are covered in Rust
// (crates/foliark-engine/tests/revisions.rs) with a stand-in applier.
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import init, * as wasm from "../wasm/foliark_wasm.js";
import { createFoliark, FoliarkError, type DocumentHandle, type Foliark } from "../src/index.js";
import { createWorkerCore } from "../src/worker-core.js";

const wasmBytes = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const pdf = new Uint8Array(readFileSync(new URL("../../../tests/fixtures/hddn/shared-vector.pdf", import.meta.url)));

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
const open = async (): Promise<DocumentHandle> => {
  const f = await createFoliark({ wasmUrl: "unused", worker });
  clients.push(f);
  return f.open(pdf.slice());
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
const region = (id: string, page: number) => ({ kind: "region" as const, id, pageIndex: page, rect: { x: 10, y: 10, width: 50, height: 20 } });

describe("commit, undo and reopen", () => {
  it("results carry the revision they were computed at, and changedPages names what moved", async () => {
    const doc = await open();
    const [text0, hits0] = [await doc.extractText(0), await doc.search("shared_vector")];
    expect([text0.revision, ...hits0.map((h) => h.revision)].every((r) => r === 0)).toBe(true);

    const first = await doc.applyChanges({ baseRevision: 0, redactions: [region("a", 1)] });
    expect(first).toMatchObject({ accepted: true, revision: 1, changedPages: [1] });
    // Text cannot change under a redaction list, so the same text is served, stamped with the new revision.
    const text1 = await doc.extractText(0);
    expect([text1.revision, text1.text, doc.revision]).toEqual([1, text0.text, 1]);
    const hits1 = await doc.search("shared_vector");
    expect(hits1.map((h) => [h.pageIndex, h.start, h.end, h.revision])).toEqual(hits0.map((h) => [h.pageIndex, h.start, h.end, 1]));

    // Undo is resubmitting the older list: a new revision, not a rewind.
    const undone = await doc.applyChanges({ baseRevision: 1, redactions: [] });
    expect(undone).toMatchObject({ accepted: true, revision: 2, changedPages: [1] });
    expect((await doc.extractText(1)).revision).toBe(2);
  });

  it("a reopened document starts again at revision 0, independent of the old handle", async () => {
    const f = await createFoliark({ wasmUrl: "unused", worker });
    clients.push(f);
    const a = await f.open(pdf.slice());
    await a.applyChanges({ baseRevision: 0, redactions: [] });
    const b = await f.open(pdf.slice());
    expect([a.revision, b.revision, a.id === b.id]).toEqual([1, 0, false]);
    expect((await b.extractText(0)).revision).toBe(0);
    expect((await a.extractText(0)).revision).toBe(1);
  });
});

describe("stale input is refused, never answered with old coordinates", () => {
  it("a stale base is STALE_REVISION and changes nothing, even when resubmitted", async () => {
    const doc = await open();
    const cs = { baseRevision: 0, redactions: [region("a", 0)] };
    expect((await doc.applyChanges(cs)).accepted).toBe(true);
    const e = await rejects(doc.applyChanges(cs)); // the same ChangeSet cannot commit twice
    expect([e.code, e.docId, e.revision, doc.revision]).toEqual(["STALE_REVISION", doc.id, 1, 1]);
    // A base from the future is refused the same way.
    expect((await rejects(doc.applyChanges({ baseRevision: 5, redactions: [] }))).code).toBe("STALE_REVISION");
    expect(doc.revision).toBe(1);
  });

  it("a text selection made at one revision cannot be redacted against another", async () => {
    const doc = await open();
    const page = await doc.extractText(0);
    const word = page.words[0];
    const range = await doc.quadsFor(0, word.start, word.end);
    await doc.applyChanges({ baseRevision: 0, redactions: [region("r", 0)] }); // the document moved on
    const stale = { baseRevision: 0, redactions: [{ kind: "text" as const, id: "t", pageIndex: 0, start: range.start, end: range.end }] };
    expect((await rejects(doc.applyChanges(stale))).code).toBe("STALE_REVISION");
    expect(doc.revision).toBe(1);
    // Rebased on the current revision the same selection resolves against the current text and is accepted.
    const ok = await doc.applyChanges({ ...stale, baseRevision: 1 });
    expect(ok).toMatchObject({ accepted: true, revision: 2 });
  });
});

describe("a long task cannot straddle a commit", () => {
  it("a search stepped across a commit fails STALE_REVISION; a fresh one carries the new revision", async () => {
    await init({ module_or_path: wasmBytes });
    const e = new wasm.Engine(undefined, 1);
    const { docId } = e.open(pdf.slice(), undefined, undefined);
    const search = e.startSearch(docId, "shared_vector", {});
    expect(e.step(search, 1).done).toBe(false); // one page, the rest pending
    const commit = e.startApplyChanges(docId, { baseRevision: 0, redactions: [] });
    expect(e.step(commit, 1_000_000).result).toMatchObject({ accepted: true, revision: 1 });
    expect(() => e.step(search, 1)).toThrow(expect.objectContaining({ code: "STALE_REVISION" }));
    const fresh = e.startSearch(docId, "shared_vector", {});
    let done;
    while (!(done = e.step(fresh, 1_000_000)).done);
    expect(done.result.length).toBeGreaterThan(0);
    expect(done.result.every((h: { revision: number }) => h.revision === 1)).toBe(true);
    e.close(docId);
  });

  it("a redaction-only commit keeps the cached layouts, so text is not re-extracted", async () => {
    await init({ module_or_path: wasmBytes });
    const e = new wasm.Engine(undefined, 1);
    const { docId } = e.open(pdf.slice(), undefined, undefined);
    const extract = (p: number) => {
      const t = e.startExtractText(docId, p, {});
      return e.step(t, 1_000_000).result;
    };
    extract(0);
    extract(1);
    const cached = e.stats();
    expect(cached.cacheEntries).toBe(2);
    const c = e.startApplyChanges(docId, { baseRevision: 0, redactions: [region("a", 1)] });
    e.step(c, 1_000_000);
    expect(e.stats()).toMatchObject({ cacheEntries: 2, cacheBytes: cached.cacheBytes });
    expect(extract(0).revision).toBe(1);
  });
});
