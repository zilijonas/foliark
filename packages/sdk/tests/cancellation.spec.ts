// Cancellation and hard recovery (spec 10.1, 10.2). The worker runs in-process over a real MessageChannel with
// the real wasm engine. A "hung" worker is simulated by swallowing requests: an in-process worker cannot block
// the thread that supervises it, so a truly synchronous hang is covered by the browser run (tests/smoke).
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as wasm from "../wasm/foliark_wasm.js";
import init from "../wasm/foliark_wasm.js";
import { createFoliark, FoliarkError, type Foliark } from "../src/index.js";
import { createWorkerCore } from "../src/worker-core.js";

const wasmBytes = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const fixture = (path: string) => new Uint8Array(readFileSync(new URL(`../../../tests/fixtures/${path}`, import.meta.url)));
const font = new Uint8Array(readFileSync(new URL("../../../crates/foliark-text/tests/data/LiberationSans-Regular.ttf", import.meta.url)));

type Req = { id: number; gen: number; op: string; args: any };
interface Fault {
  /** From the first request for which this is true, the worker is stuck: it never reads or answers anything again. */
  hang?: (m: Req) => boolean;
  /** The worker does the op but its answer is lost. */
  loseAnswer?: (m: Req) => boolean;
  /** Called with the requests each worker instance received. */
  seen?: Req[];
  /** With `hang`: instead of going silent, report progress every `ms` for `for` ms and then answer. */
  chatter?: { ms: number; for: number };
}

/** `stepBudget` 1 makes every page its own step, so a task spans many event-loop turns like a long page does. */
const worker = (stepBudget?: number, fault: Fault = {}) => (): Worker => {
  const { port1, port2 } = new MessageChannel();
  let stuck = false;
  const lost = new Set<number>();
  const core = createWorkerCore({
    post: (m, transfer = []) => {
      if (lost.has((m as { id?: number }).id ?? -1)) return;
      port2.postMessage(m, { transfer });
    },
    loadWasm: () => init({ module_or_path: wasmBytes }).then(() => undefined),
  }, stepBudget);
  port2.onmessage = (e) => {
    const m = e.data as Req;
    if (m.op) fault.seen?.push(m);
    if (stuck) return;
    if (m.op && fault.hang?.(m)) {
      const { chatter } = fault;
      if (!chatter) return void (stuck = true);
      // A long op that keeps reporting progress, then answers an empty result.
      const end = Date.now() + chatter.for;
      const timer = setInterval(() => {
        const done = Date.now() >= end;
        if (done) clearInterval(timer);
        port2.postMessage(done ? { v: 1, id: m.id, ok: true, result: [] } : { v: 1, id: m.id, progress: { phase: "extract", done: 1, total: 2 } });
      }, chatter.ms);
      return;
    }
    if (m.op && fault.loseAnswer?.(m)) lost.add(m.id);
    core.onMessage(m);
  };
  return Object.assign(port1, { terminate: () => (port1.close(), port2.close()) }) as unknown as Worker;
};

const clients: Foliark[] = [];
const client = async (o: Partial<Parameters<typeof createFoliark>[0]> = {}) => {
  const f = await createFoliark({ wasmUrl: "unused", worker: worker(), ...o });
  clients.push(f);
  return f;
};
afterEach(async () => {
  vi.restoreAllMocks();
  await Promise.all(clients.splice(0).map((c) => c.dispose()));
});

const settle = async (p: Promise<unknown>): Promise<unknown> => p.then((v) => v, (e) => e);
const rejects = async (p: Promise<unknown>) => {
  const e = await settle(p);
  expect(e).toBeInstanceOf(FoliarkError);
  return e as FoliarkError;
};
const never = (ms: number) => new Promise<string>((r) => setTimeout(() => r("unsettled"), ms));
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const tick = () => sleep(0);
const reasons = (f: Foliark) => {
  const seen: { generation: number; reason: string }[] = [];
  f.onReset((e) => seen.push(e));
  return seen;
};
const noop = (base: number) => ({ baseRevision: base, redactions: [] });

describe("hang watchdog", () => {
  const HANG = 80;
  /** The first `op` request hangs its worker; the replacement worker is healthy. */
  const once = (test: (m: Req) => boolean) => {
    let fired = false;
    return (m: Req) => !fired && test(m) && (fired = true);
  };
  const hungOn = (op: string): Fault => ({ hang: once((m) => m.op === op) });

  it("terminates a worker that goes silent during an op: pending calls reject WORKER_RESET, old handles are gone, fonts are re-sent", async () => {
    const seen: Req[] = [];
    const f = await client({ hangTimeoutMs: HANG, worker: worker(undefined, { ...hungOn("extractText"), seen }) });
    const resets = reasons(f);
    await f.registerFonts([{ bytes: font, family: "Liberation Sans", weight: 400, style: "normal" }]);
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    const started = Date.now();
    const stuck = rejects(doc.extractText(0));
    const queued = rejects(doc.search("anything")); // waits behind the stuck op in the worker
    const [a, b] = await Promise.all([stuck, queued]);
    expect([a.code, a.recoverable, b.code]).toEqual(["WORKER_RESET", true, "WORKER_RESET"]);
    expect(Date.now() - started).toBeLessThan(HANG * 20);
    expect([f.generation, resets]).toEqual([2, [{ generation: 2, reason: "hang" }]]);
    expect((await rejects(doc.pages())).code).toBe("DOCUMENT_GONE");
    // The new worker was initialised and got the client-held font bytes before anything else.
    await vi.waitFor(() => expect(seen.filter((m) => m.gen === 2).map((m) => m.op).slice(0, 2)).toEqual(["init", "registerFonts"]));
    expect(seen.find((m) => m.gen === 2 && m.op === "registerFonts")!.args.fonts).toHaveLength(1);
    // And it works.
    const again = await f.open(fixture("hddn/rotation-crop.pdf"));
    expect((await again.extractText(0)).revision).toBe(0);
    expect(again.generation).toBe(2);
  });

  it("does not terminate a long op that keeps reporting progress", async () => {
    const f = await client({ hangTimeoutMs: HANG, worker: worker(undefined, { ...hungOn("search"), chatter: { ms: HANG / 4, for: HANG * 4 } }) });
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    expect(await doc.search("x")).toEqual([]);
    expect(f.generation).toBe(1);
  });

  it("counts silence only while a request is unanswered", async () => {
    const f = await client({ hangTimeoutMs: HANG });
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    await sleep(HANG * 3);
    expect(f.generation).toBe(1);
    expect((await doc.extractText(0)).pageIndex).toBe(0); // still the same worker, still alive
  });

  it("requests queued behind a stuck op do not extend its deadline", async () => {
    const f = await client({ hangTimeoutMs: HANG * 2, worker: worker(undefined, hungOn("extractText")) });
    const resets = reasons(f);
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    const started = Date.now();
    const stuck = rejects(doc.extractText(0));
    for (let i = 0; i < 4; i++) {
      await sleep(HANG / 2);
      void settle(doc.search(`q${i}`));
    }
    await stuck;
    expect(Date.now() - started).toBeLessThan(HANG * 4);
    expect(resets).toHaveLength(1);
  });

  it("a lost acknowledgment is a reset, never an unchanged document", async () => {
    // The worker commits the ChangeSet and then goes silent before it answers.
    const f = await client({ hangTimeoutMs: HANG, worker: worker(undefined, { loseAnswer: once((m) => m.op === "applyChanges") }) });
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    // loseAnswer drops the result only; nothing else tells the client, so the watchdog has to.
    const lost = await rejects(doc.applyChanges(noop(0)));
    expect([lost.code, lost.recoverable]).toEqual(["WORKER_RESET", true]);
    expect(lost.code).not.toBe("CANCELLED");
    expect(doc.revision).toBe(0); // the handle never saw a commit, and it is dead: nothing can read "unchanged"
    expect((await rejects(doc.extractText(0))).code).toBe("DOCUMENT_GONE");
    // The caller reopens its bytes and re-applies its ChangeSet on the new generation.
    const again = await f.open(fixture("hddn/rotation-crop.pdf"));
    expect(await again.applyChanges(noop(0))).toMatchObject({ accepted: true, revision: 1 });
  });

  it("settles every pending promise when the worker is replaced", async () => {
    const f = await client({ hangTimeoutMs: HANG, worker: worker(undefined, hungOn("renderPage")) });
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    const ctl = new AbortController();
    const calls = [
      doc.renderPage(0), doc.extractText(0), doc.search("x"), doc.hitTest(0, { x: 1, y: 1 }),
      doc.extractText(1, { signal: ctl.signal }), f.registerFonts([{ bytes: font, family: "L", weight: 400, style: "normal" }]),
    ];
    ctl.abort(); // a cancel for a request the stuck worker never reads
    const all = await Promise.race([Promise.allSettled(calls), never(HANG * 30)]);
    expect(all).not.toBe("unsettled");
    for (const r of all as PromiseSettledResult<unknown>[]) {
      expect(r.status).toBe("rejected");
      expect(((r as PromiseRejectedResult).reason as FoliarkError).code).toBe("WORKER_RESET");
    }
  });

  it("dispose while an op is stuck settles it CLIENT_DISPOSED and no reset follows", async () => {
    const f = await client({ hangTimeoutMs: HANG, worker: worker(undefined, hungOn("renderPage")) });
    const resets = reasons(f);
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    const stuck = rejects(doc.renderPage(0));
    await f.dispose();
    expect((await stuck).code).toBe("CLIENT_DISPOSED");
    await sleep(HANG * 3);
    expect(resets).toEqual([]);
  });

  it("rejects a bad hangTimeoutMs and releases the worker of a client that failed to start", async () => {
    for (const bad of [0, -1, NaN, Infinity]) {
      expect((await rejects(createFoliark({ wasmUrl: "unused", worker: worker(), hangTimeoutMs: bad }))).code).toBe("INVALID_ARGUMENT");
    }
    // A worker that never initialises: the watchdog resets it, and the failed start rejects instead of hanging.
    const e = await rejects(createFoliark({ wasmUrl: "unused", hangTimeoutMs: HANG, worker: worker(undefined, hungOn("init")) }));
    expect(e.code).toBe("WORKER_RESET");
  });
});

describe("crash and out-of-memory recovery", () => {
  it("a worker error is a crash, one that names memory is oom; a fatal event follows the same rule", async () => {
    const workers: Worker[] = [];
    const f = await client({ worker: () => workers[workers.push(worker()()) - 1] });
    const resets = reasons(f);
    const fail = (w: Worker, message: string) => (w as unknown as { onerror: (e: unknown) => void }).onerror({ message });
    const generation = async (n: number) => vi.waitFor(() => expect(f.generation).toBe(n));
    fail(workers.at(-1)!, "boom");
    await generation(2);
    fail(workers.at(-1)!, "RangeError: WebAssembly.Memory.grow(): Maximum memory size exceeded");
    await generation(3);
    const current = workers.at(-1) as unknown as MessagePort;
    current.onmessage?.({ data: { v: 1, event: "fatal", gen: 3, reason: "out of memory" } } as MessageEvent);
    await generation(4);
    expect(resets.map((r) => r.reason)).toEqual(["crash", "oom", "oom"]);
    // A late error from a terminated worker is not about the current one.
    fail(workers[0], "late");
    await sleep(30);
    expect(f.generation).toBe(4);
  });

  it("an allocation failure inside the engine answers the op OUT_OF_MEMORY, then replaces the worker", async () => {
    const f = await client();
    const resets = reasons(f);
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    vi.spyOn(wasm.Engine.prototype, "pages").mockImplementation(() => {
      throw new RangeError("WebAssembly.Memory.grow(): Maximum memory size exceeded");
    });
    const e = await rejects(doc.pages());
    expect([e.code, e.recoverable]).toEqual(["OUT_OF_MEMORY", false]);
    await vi.waitFor(() => expect(resets).toEqual([{ generation: 2, reason: "oom" }]));
    vi.restoreAllMocks();
    expect((await rejects(doc.pages())).code).toBe("DOCUMENT_GONE");
    expect((await (await f.open(fixture("hddn/rotation-crop.pdf"))).pages()).length).toBe(4);
  });
});

describe("cooperative cancellation", () => {
  const stepped = (fault?: Fault) => client({ worker: worker(1, fault) });

  it("cancels a queued op before it starts and leaves the revision alone", async () => {
    const f = await stepped();
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    const slow = doc.search("a"); // 4 steps, each a turn of the event loop
    const ctl = new AbortController();
    const queued = doc.applyChanges(noop(0), { signal: ctl.signal });
    ctl.abort();
    expect((await rejects(queued)).code).toBe("CANCELLED");
    await slow;
    expect(doc.revision).toBe(0);
    // Nothing was committed, so revision 0 is still the base.
    expect(await doc.applyChanges(noop(0))).toMatchObject({ accepted: true, revision: 1 });
  });

  it("an abort between two steps drops the task: CANCELLED, and the next op is unaffected", async () => {
    const f = await stepped();
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    const ctl = new AbortController();
    const running = doc.search("a", { signal: ctl.signal });
    await tick();
    ctl.abort();
    expect((await rejects(running)).code).toBe("CANCELLED");
    expect((await doc.search("a")).length).toBeGreaterThanOrEqual(0);
    expect(f.generation).toBe(1);
  });

  it("an already aborted signal never reaches the worker", async () => {
    const seen: Req[] = [];
    const f = await stepped({ seen });
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    const before = seen.length;
    const ctl = new AbortController();
    ctl.abort();
    expect((await rejects(doc.extractText(0, { signal: ctl.signal }))).code).toBe("CANCELLED");
    expect(seen.length).toBe(before);
  });

  it("an abort racing the commit settles one way or the other, and the engine and the handle always agree", async () => {
    const f = await stepped();
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    const outcomes = { committed: 0, cancelled: 0 };
    for (let hops = 0; hops < 10; hops++) {
      const ctl = new AbortController();
      const before = doc.revision;
      const call = doc.applyChanges(noop(before), { signal: ctl.signal });
      for (let i = 0; i < hops; i++) await tick();
      ctl.abort();
      const out = await settle(call);
      if (out instanceof FoliarkError) {
        expect(out.code).toBe("CANCELLED");
        expect(doc.revision).toBe(before);
        outcomes.cancelled++;
      } else {
        expect(out).toMatchObject({ accepted: true, revision: before + 1 });
        expect(doc.revision).toBe(before + 1); // a committed result is returned, never CANCELLED
        outcomes.committed++;
      }
      // The engine is at exactly the revision the handle reports: this base is accepted, one past it is not.
      const rev = doc.revision;
      expect((await rejects(doc.applyChanges(noop(rev + 1)))).code).toBe("STALE_REVISION");
      expect(await doc.applyChanges(noop(rev))).toMatchObject({ accepted: true, revision: rev + 1 });
    }
    expect(outcomes.committed + outcomes.cancelled).toBe(10);
  });

  it("an abort that arrives after the answer is ignored and does not poison the next request", async () => {
    const f = await stepped();
    const doc = await f.open(fixture("hddn/rotation-crop.pdf"));
    const ctl = new AbortController();
    const r = await doc.applyChanges(noop(0), { signal: ctl.signal });
    ctl.abort();
    await tick();
    expect(r.accepted).toBe(true);
    expect(await doc.applyChanges(noop(1))).toMatchObject({ accepted: true, revision: 2 });
  });
});
