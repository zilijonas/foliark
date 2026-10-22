// Model-based test of the SDK state machine (spec 1, 4, 10.1, 10.2). Seeded random sequences of open, extractText,
// search, applyChanges, prepareExport, export, close and hard restart (with aborts at random event-loop hops) run
// against the real SDK over the real wasm engine (in-process worker over a real MessageChannel, small stepBudget
// so tasks interleave) and against a pure model of the spec. After every group of concurrent calls the model and
// the engine must agree on: every outcome (value or error code), the revision of every open document, the single
// held plan, and the bytes that crossed the wire.
//
// Outcomes the spec leaves open are modeled as unknown, never guessed: an aborted call is either CANCELLED (nothing
// happened) or completed (the result is then checked against the model); a call in flight at a hard reset either
// finished before it (checked) or is WORKER_RESET (the op may or may not have run, but the worker's memory is gone
// either way, so the model resets and a reopened document must be at revision 0).
//
// SEED=n replays sequence n (and only it); SEQUENCES=n changes the count (default 200). A failure is shrunk (drop
// groups, then ops) and printed with its seed.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import init from "../wasm/foliark_wasm.js";
import { createFoliark, FoliarkError, type DocumentHandle, type Foliark } from "../src/index.js";
import type { ExportPlan } from "../src/generated/ExportPlan.js";
import { createWorkerCore } from "../src/worker-core.js";

const wasmBytes = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const FIXTURES = ["hddn/metadata-xmp.pdf", "hddn/rotation-crop.pdf"];
const pdfs = FIXTURES.map((f) => new Uint8Array(readFileSync(new URL(`../../../tests/fixtures/${f}`, import.meta.url))));
/** Default `maxOpenDocuments` (spec 9). */
const MAX_OPEN = 4;
/** One event-loop hop, the same granularity as the worker's own yield between steps (a timer is far coarser). */
const hop = () =>
  new Promise<void>((resolve) => {
    const { port1, port2 } = new MessageChannel();
    port1.onmessage = () => (port1.close(), resolve());
    port2.postMessage(null);
  });

// ---- sequences -------------------------------------------------------------------------------------------------
type Op =
  | { k: "open"; fix: number }
  | { k: "extract"; doc: number; page: number; abort?: number }
  | { k: "search"; doc: number; abort?: number }
  | { k: "apply"; doc: number; delta: number; regions: { page: number; x: number }[] | null; badPage?: boolean; abort?: number }
  | { k: "prepare"; doc: number; abort?: number }
  | { k: "export"; doc: number; plan: number }
  | { k: "close"; doc: number };
interface Group {
  ops: Op[];
  /** Hard restart after this many event-loop hops, with the group still in flight. */
  restart?: number;
}
interface Sequence {
  stepBudget: number;
  groups: Group[];
}

function prng(seed: number) {
  let a = seed >>> 0;
  const next = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const int = (n: number) => Math.floor(next() * n);
  const pick = <T>(w: [T, number][]): T => {
    let r = next() * w.reduce((s, [, x]) => s + x, 0);
    for (const [v, x] of w) if ((r -= x) < 0) return v;
    return w[0][0];
  };
  return { next, int, pick };
}

function generate(seed: number): Sequence {
  const r = prng(seed);
  const groups: Group[] = [];
  for (let g = 0, n = 6 + r.int(11); g < n; g++) {
    const ops: Op[] = [];
    const size = g === 0 ? 1 : r.pick([[1, 3], [2, 3], [3, 3], [4, 1]] as [number, number][]);
    const restart = g > 0 && r.next() < 0.12 ? r.int(7) : undefined;
    for (let i = 0; i < size; i++) {
      const last = i === size - 1;
      const doc = r.next() < 0.8 ? r.int(8) * 2 : r.int(8) * 2 + 1; // even slots prefer open documents, see `live` below
      const abort = restart === undefined && r.next() < 0.3 ? r.int(7) : undefined;
      const kind = g === 0 ? "open" : r.pick([["open", 10], ["extract", 12], ["search", 12], ["apply", 24], ["prepare", 14], ["export", 18], ["close", 6]] as ["open" | "extract" | "search" | "apply" | "prepare" | "export" | "close", number][]);
      if (kind === "open") ops.push({ k: "open", fix: r.int(FIXTURES.length) });
      else if (kind === "extract") ops.push({ k: "extract", doc, page: 0, abort });
      else if (kind === "search") ops.push({ k: "search", doc, abort });
      else if (kind === "apply") {
        const regions = r.next() < 0.25 ? null : Array.from({ length: r.int(4) }, () => ({ page: r.int(4), x: 10 + r.int(100) }));
        ops.push({ k: "apply", doc, delta: r.pick([[0, 6], [-1, 1], [1, 1], [2, 1]] as [number, number][]), regions, badPage: r.next() < 0.1 || undefined, abort: last ? abort : undefined });
      } else if (kind === "prepare") ops.push({ k: "prepare", doc, abort: last ? abort : undefined });
      else if (kind === "export") ops.push({ k: "export", doc, plan: r.int(8) });
      else ops.push({ k: "close", doc });
    }
    // A slow op first leaves the rest queued behind it: that is where cancel-before-start happens.
    if (size > 1 && r.next() < 0.4) ops[0] = { k: "search", doc: r.int(16) };
    groups.push({ ops, restart });
  }
  return { stepBudget: r.pick([[1, 3], [7, 1], [100_000, 1]] as [number, number][]), groups };
}

// ---- the real SDK, with a tap on the wire ------------------------------------------------------------------------
interface Tap {
  ops: Map<number, string>;
  answers: Map<number, number>;
  exportBytes: number;
  /** Answers that carried a value though they were errors, or an error without the typed shape. */
  malformed: string[];
}
function harness(stepBudget: number) {
  const taps: Tap[] = [];
  const make = (): Worker => {
    const { port1, port2 } = new MessageChannel();
    const tap: Tap = { ops: new Map(), answers: new Map(), exportBytes: 0, malformed: [] };
    taps.push(tap);
    const core = createWorkerCore({
      post: (m, transfer = []) => {
        const a = m as { id?: number; ok?: boolean; result?: unknown };
        if (a.id !== undefined && a.ok !== undefined) {
          tap.answers.set(a.id, (tap.answers.get(a.id) ?? 0) + 1);
          if (tap.ops.get(a.id) === "export") {
            if (a.ok && a.result instanceof ArrayBuffer) tap.exportBytes++;
            else if (a.ok || "result" in a) tap.malformed.push(`export ${a.id} answered ${JSON.stringify(a).slice(0, 80)}`);
          }
        }
        port2.postMessage(m, { transfer });
      },
      loadWasm: () => init({ module_or_path: wasmBytes }).then(() => undefined),
    }, stepBudget);
    port2.onmessage = (e) => {
      const m = e.data as { id?: number; op?: string };
      if (m.op && m.id !== undefined) tap.ops.set(m.id, m.op);
      core.onMessage(m);
    };
    return Object.assign(port1, { terminate: () => (port1.close(), port2.close()) }) as unknown as Worker;
  };
  return { make, taps };
}

// ---- the model ---------------------------------------------------------------------------------------------------
type Region = { kind: "region"; id: string; pageIndex: number; rect: { x: number; y: number; width: number; height: number } };
interface MDoc {
  fix: number;
  pages: number;
  rev: number;
  list: Region[];
  /** The SDK handle knows it is closed (set when a close was acknowledged, or the worker was replaced). */
  handleClosed: boolean;
  /** The engine still holds it. */
  engineOpen: boolean;
  handle: DocumentHandle;
}
interface MPlan {
  doc: MDoc;
  rev: number;
  regions: number;
  consumed: boolean;
  api?: ExportPlan;
}
type Expect = { ok: true; check?: (v: any) => void } | { code: string };
interface Issued {
  op: Op;
  promise: Promise<unknown>;
  expect: Expect;
  /** True when no worker is involved (the handle already knows the document is gone). */
  clientSide: boolean;
  /** Applies the effect; deferred until the outcome is known for an op that may be aborted. */
  commit?: (v: unknown) => void;
  committed: boolean;
}

const seen: Record<string, number> = {};
const count = (k: string) => void (seen[k] = (seen[k] ?? 0) + 1);
class Failure extends Error {}
const fail = (m: string): never => {
  throw new Failure(m);
};

async function run(seq: Sequence): Promise<void> {
  const { make, taps } = harness(seq.stepBudget);
  const f: Foliark = await createFoliark({ wasmUrl: "unused", worker: make, hangTimeoutMs: 60_000 });
  let gen = 1;
  let docs: MDoc[] = [];
  let held: MPlan | null = null;
  const plansEver: MPlan[] = [];
  let nextRegion = 0;
  let exportsOk = 0;
  let lostExports = 0;
  let openCount = 0; // documents the engine holds
  const settled = new WeakMap<Promise<unknown>, { v?: unknown; e?: unknown }>();
  try {
    for (const [gi, group] of seq.groups.entries()) {
      const here = `group ${gi}`;
      const live = docs.filter((d) => d.handle);
      const fresh: MDoc[] = [];
      const closedNow: MDoc[] = [];
      const issued: Issued[] = [];
      const gone = (d: MDoc) => d.handleClosed;
      const abortHops: [Issued, number, AbortController][] = [];

      for (const op of group.ops) {
        const ctl = new AbortController();
        // Even slots pick among documents the handle still believes open, odd ones among all (stale handles).
        const pool = op.k !== "open" && "doc" in op && op.doc % 2 === 0 && live.some((x) => !x.handleClosed) ? live.filter((x) => !x.handleClosed) : live;
        const d = "doc" in op && pool.length ? pool[op.doc % pool.length] : undefined;
        if ("doc" in op && !d) continue; // nothing to act on: the op drops out (keeps shrunk sequences valid)
        const i: Issued = { op, promise: undefined as never, expect: { ok: true }, clientSide: false, committed: false };
        const signal = "abort" in op && op.abort !== undefined ? ctl.signal : undefined;
        const stateful = op.k === "apply" || op.k === "prepare";
        const needsOutcome = signal && stateful; // an aborted commit may or may not have happened
        const dead = d && (gone(d) || !d.engineOpen);
        const rev = d?.rev ?? 0; // expectations are about the state at issue time, not at check time
        if (d && gone(d)) i.clientSide = true;
        // The effect on the model of this op, once it is known to have run.
        const effect = (apply: (v: any) => void) => {
          i.commit = (v) => {
            i.committed = true;
            apply(v);
          };
          if (!needsOutcome) i.commit(undefined);
        };
        switch (op.k) {
          case "open": {
            i.promise = f.open(pdfs[op.fix].slice());
            if (openCount >= MAX_OPEN) {
              i.expect = { code: "LIMIT_EXCEEDED" };
              break;
            }
            openCount++;
            i.commit = (h: any) => {
              i.committed = true;
              fresh.push({ fix: op.fix, pages: 0, rev: 0, list: [], handleClosed: false, engineOpen: true, handle: h });
            };
            break;
          }
          case "extract":
            i.promise = d!.handle.extractText(op.page, { signal });
            i.expect = dead ? { code: "DOCUMENT_GONE" } : { ok: true, check: (t) => expect(t.revision, "extract revision").toBe(rev) };
            break;
          case "search":
            i.promise = d!.handle.search("a", { signal });
            i.expect = dead ? { code: "DOCUMENT_GONE" } : { ok: true, check: (h) => expect(h.every((x: any) => x.revision === rev), "hit revisions").toBe(true) };
            break;
          case "apply": {
            const regions: Region[] | null = op.regions
              ? op.regions.map((r) => ({
                  kind: "region" as const,
                  id: `r${nextRegion++}`,
                  pageIndex: op.badPage ? 999 : r.page % Math.max(1, d!.pages),
                  rect: { x: r.x, y: 10, width: 40, height: 12 },
                }))
              : null;
            const delta = op.delta < 0 && rev === 0 ? 3 : op.delta; // a negative base is a protocol error, not a stale one
            const base = rev + delta;
            i.promise = d!.handle.applyChanges({ baseRevision: base, ...(regions ? { redactions: regions } : {}) }, { signal });
            if (dead) i.expect = { code: "DOCUMENT_GONE" };
            else if (delta !== 0) i.expect = { code: "STALE_REVISION" };
            else if (op.badPage && regions?.length) i.expect = { ok: true, check: (r) => expect([r.accepted, r.revision], "rejected change must not move the revision").toEqual([false, rev]) };
            else {
              const before = d!.list;
              const after = regions ?? before;
              const changed = new Set<number>();
              for (let p = 0; p < d!.pages; p++) {
                const pick = (l: Region[]) => JSON.stringify(l.filter((x) => x.pageIndex === p));
                if (pick(before) !== pick(after)) changed.add(p);
              }
              i.expect = {
                ok: true,
                check: (r) => expect([r.accepted, r.revision, r.changedPages], "commit result").toEqual([true, rev + 1, [...changed].sort((a, b) => a - b)]),
              };
              effect(() => {
                d!.rev++;
                d!.list = after;
                held = null;
              });
            }
            break;
          }
          case "prepare": {
            i.promise = d!.handle.prepareExport({ mode: "native", signal });
            if (dead) {
              i.expect = { code: "DOCUMENT_GONE" };
              break;
            }
            const plan: MPlan = { doc: d!, rev: d!.rev, regions: d!.list.length, consumed: false };
            i.expect = {
              ok: true,
              check: (p: ExportPlan) => {
                plan.api = p;
                expect([p.docId, p.revision, p.mode, p.generation], "plan binding").toEqual([d!.handle.id, plan.rev, "native", gen]);
                expect(p.report.redaction.regions, "plan reports the redaction list it was built from").toBe(plan.regions);
              },
            };
            effect(() => {
              held = plan;
              plansEver.push(plan);
            });
            break;
          }
          case "export": {
            const plan = plansEver.length ? plansEver[op.plan % plansEver.length] : undefined;
            if (!plan?.api) continue;
            i.promise = d!.handle.export(plan.api);
            if (dead) i.expect = { code: "DOCUMENT_GONE" };
            else if (held !== plan || plan.doc !== d) i.expect = { code: "STALE_PLAN" };
            else if (plan.consumed) i.expect = { code: "PLAN_CONSUMED" };
            else {
              i.expect = { ok: true, check: (b: Uint8Array) => {
                exportsOk++;
                expect([new TextDecoder().decode(b.slice(0, 5)), b.byteLength], "exported bytes").toEqual(["%PDF-", plan.api!.byteLength]);
              } };
              plan.consumed = true;
            }
            break;
          }
          case "close": {
            i.promise = d!.handle.close();
            i.expect = { ok: true };
            if (!gone(d!)) {
              closedNow.push(d!);
              held = null; // the engine frees the held plan on any close
              if (d!.engineOpen) openCount--;
              d!.engineOpen = false;
            }
            break;
          }
        }
        issued.push(i);
        if (signal) abortHops.push([i, (op as { abort: number }).abort, ctl]);
        void i.promise.then((v) => settled.set(i.promise, { v }), (e) => settled.set(i.promise, { e }));
      }

      // Interleave: aborts at their hop, and a hard restart in the middle of the flight.
      const all = Promise.all(issued.map((i) => i.promise.then(() => undefined, () => undefined)));
      const maxHops = Math.max(group.restart ?? 0, ...abortHops.map(([, h]) => h), 0);
      for (let h = 0; h <= maxHops; h++) {
        for (const [, hops, ctl] of abortHops) if (hops === h) ctl.abort();
        if (group.restart === h) void f.restart().catch(() => undefined);
        await hop();
      }
      const timeout = new Promise<string>((r) => setTimeout(() => r("unsettled"), 8000));
      if ((await Promise.race([all, timeout])) === "unsettled") fail(`${here}: a promise never settled: ${issued.filter((i) => !settled.has(i.promise)).map((i) => i.op.k)}`);

      // Compare outcomes, in issue order.
      let sawReset = false;
      for (const i of issued) {
        const out = settled.get(i.promise)!;
        const tag = `${here} ${JSON.stringify(i.op)}`;
        const err = out.e as FoliarkError | undefined;
        if (out.e !== undefined && !(err instanceof FoliarkError)) fail(`${tag}: untyped rejection ${String(out.e)}`);
        if (err?.code === "WORKER_RESET") {
          if (group.restart === undefined || i.clientSide) fail(`${tag}: WORKER_RESET without a reset of the worker`);
          if (!err!.recoverable) fail(`${tag}: WORKER_RESET must be recoverable`);
          sawReset = true;
          count("reset");
          if (i.op.k === "export") lostExports++; // its bytes may have crossed the wire: a lost acknowledgment
          continue;
        }
        if (sawReset && !i.clientSide) fail(`${tag}: answered normally after an earlier call of the same worker was reset`);
        if (err?.code === "CANCELLED") {
          if (!("abort" in i.op) || i.op.abort === undefined) fail(`${tag}: CANCELLED without an abort`);
          count(`cancelled ${i.op.k}`);
          continue; // nothing happened: the effect is simply never committed
        }
        const want = i.expect;
        if ("code" in want) {
          if (err?.code !== want.code) fail(`${tag}: expected ${want.code}, got ${err ? err.code : "success"}`);
        } else {
          if (err) fail(`${tag}: expected success, got ${err.code}: ${err.message}`);
          try {
            want.check?.(out.v);
          } catch (e) {
            fail(`${tag}: ${(e as Error).message}`);
          }
        }
        count(`${i.op.k} ${err ? err.code : "ok"}${"abort" in i.op && i.op.abort !== undefined ? " (abortable)" : ""}`);
        if (!err && !i.committed) i.commit?.(out.v);
        else if (i.op.k === "close") fail(`${tag}: close must not fail`);
      }
      if (group.restart !== undefined) {
        gen++;
        held = null;
        openCount = 0;
        for (const d of docs.concat(fresh)) {
          d.handleClosed = true;
          d.engineOpen = false;
        }
        for (const d of fresh) {
          d.pages = (await d.handle.info()).pageCount;
          docs.push(d);
        }
        if (f.generation !== gen) fail(`${here}: generation ${f.generation}, expected ${gen}`);
        for (const d of live.concat(fresh)) {
          for (const call of [() => d.handle.extractText(0), () => d.handle.applyChanges({ baseRevision: 0 })]) {
            const e = await call().then(() => undefined, (x) => x as FoliarkError);
            if (e?.code !== "DOCUMENT_GONE") fail(`${here}: old handle ${d.handle.id} answered ${e ? e.code : "success"} after the reset`);
          }
        }
        continue;
      }
      for (const d of closedNow) d.handleClosed = true;
      for (const d of fresh) {
        d.pages = (await d.handle.info()).pageCount;
        docs.push(d);
      }

      // Probe the engine's truth: revision of every open document, from the engine's own stamp.
      for (const d of docs) {
        if (d.handleClosed) continue;
        const stamp = (await d.handle.extractText(0)).revision;
        if (stamp !== d.rev || d.handle.revision !== d.rev) fail(`${here}: doc ${d.handle.id} engine revision ${stamp}, handle ${d.handle.revision}, model ${d.rev}`);
      }
    }

    // Wire: each request answered at most once; the live worker answered everything it received; bytes crossed
    // the wire exactly once per successful export and never with an error.
    const bad = taps.flatMap((t) => t.malformed);
    if (bad.length) fail(`wire: ${bad.join("; ")}`);
    for (const [wi, t] of taps.entries()) {
      for (const [id, n] of t.answers) if (n !== 1) fail(`wire: worker ${wi} answered request ${id} ${n} times`);
      if (wi === taps.length - 1) for (const id of t.ops.keys()) if (t.answers.get(id) !== 1) fail(`wire: live worker never answered request ${id} (${t.ops.get(id)})`);
    }
    const bytes = taps.reduce((s, t) => s + t.exportBytes, 0);
    if (bytes < exportsOk || bytes > exportsOk + lostExports) fail(`wire: ${bytes} byte answers for ${exportsOk} successful exports (+${lostExports} lost)`);
  } finally {
    await f.dispose();
  }
}

// ---- driver: run, shrink, report ---------------------------------------------------------------------------------
const attempt = (seq: Sequence) => run(seq).then(() => undefined, (e: Error) => e);

async function shrink(seq: Sequence, firstError: Error): Promise<{ seq: Sequence; error: Error }> {
  let best = seq;
  let error = firstError;
  for (let changed = true; changed; ) {
    changed = false;
    const candidates: Sequence[] = [];
    best.groups.forEach((_, gi) => candidates.push({ ...best, groups: best.groups.filter((_, j) => j !== gi) }));
    best.groups.forEach((g, gi) =>
      g.ops.forEach((_, oi) => {
        if (g.ops.length > 1) candidates.push({ ...best, groups: best.groups.map((x, j) => (j === gi ? { ...x, ops: x.ops.filter((_, k) => k !== oi) } : x)) });
      }),
    );
    best.groups.forEach((g, gi) => {
      if (g.restart !== undefined) candidates.push({ ...best, groups: best.groups.map((x, j) => (j === gi ? { ...x, restart: undefined } : x)) });
    });
    for (const c of candidates) {
      const e = await attempt(c);
      if (e) {
        best = c;
        error = e;
        changed = true;
        break;
      }
    }
  }
  return { seq: best, error };
}

describe("SDK state machine against the spec model", () => {
  it("seeded sequences of open, read, abort, commit, plan, export, close and restart agree with the model", async () => {
    const only = process.env.SEED === undefined ? undefined : Number(process.env.SEED);
    const count = Number(process.env.SEQUENCES ?? 500);
    const seeds = only === undefined ? Array.from({ length: count }, (_, i) => i + 1) : [only];
    const started = Date.now();
    let steps = 0;
    for (const seed of seeds) {
      const seq = generate(seed);
      steps += seq.groups.reduce((s, g) => s + g.ops.length, 0);
      const e = await attempt(seq);
      if (e) {
        const small = await shrink(seq, e);
        throw new Error(`SEED=${seed} failed: ${small.error.message}\nminimal sequence (stepBudget ${small.seq.stepBudget}):\n${JSON.stringify(small.seq.groups, null, 1)}`);
      }
    }
    console.log(`state-machine: ${seeds.length} sequences, ${steps} ops, ${Date.now() - started} ms`, JSON.stringify(seen));
    expect(Date.now() - started).toBeLessThan(60_000);
  }, 120_000);
});
