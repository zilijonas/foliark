// The worker's logic, separate from `self` so vitest can run it in-process over a real MessageChannel.
// One wasm `Engine` per worker, many documents, one request at a time in arrival order (spec 10.1).
// Long ops are Rust tasks driven step by step: between steps the loop yields to the event loop (so a
// cancel message can arrive) and posts progress. Every request is validated against spec 10.4.
import * as wasm from "../wasm/foliark_wasm.js";
import type { OpName } from "./generated/OpName.js";
import type { Progress } from "./generated/Progress.js";
import {
  asFoliarkError, checkRequest, errRes, FoliarkError, isCtl, isErrorCode, okRes, postTransfer,
} from "./protocol.js";

/** Spec 10.1: work units per step. */
export const STEP_BUDGET = 100_000;

export interface WorkerHost {
  post(message: unknown, transfer?: ArrayBuffer[]): void;
  /** Instantiates the wasm module (fetch in the real worker, bytes in tests). */
  loadWasm(url: string): Promise<void>;
}

type Args = Record<string, any>;
type Out = { result: unknown; transfer?: ArrayBuffer[] };

const isRec = (x: unknown): x is Record<string, unknown> => typeof x === "object" && x !== null;
/** Drop `undefined` values: serde-wasm-bindgen reads a present key as a value, not as "use the default". */
const defined = (o: Record<string, unknown>) => Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined));
const yieldToEventLoop = () =>
  new Promise<void>((resolve) => {
    const { port1, port2 } = new MessageChannel();
    port1.onmessage = () => {
      port1.close();
      resolve();
    };
    port2.postMessage(null);
  });

/** The wasm facade throws `{ code, message, details? }`; anything else is an engine bug. */
function fromWasm(e: unknown): FoliarkError {
  if (e instanceof FoliarkError) return e;
  if (isRec(e) && isErrorCode(e.code) && typeof e.message === "string") {
    const recoverable = !["INTERNAL", "PROTOCOL_ERROR", "OUT_OF_MEMORY"].includes(e.code);
    return new FoliarkError(e.code, e.message, { details: e.details as FoliarkError["details"], recoverable });
  }
  return asFoliarkError(e);
}

/** Exactly-sized, transferable copy of bytes the facade handed out. */
const ownBuffer = (b: Uint8Array): ArrayBuffer =>
  b.byteOffset === 0 && b.byteLength === b.buffer.byteLength ? (b.buffer as ArrayBuffer) : (b.slice().buffer as ArrayBuffer);

export function createWorkerCore(host: WorkerHost, stepBudget = STEP_BUDGET) {
  let generation: number | undefined;
  let engine: wasm.Engine | undefined;
  let queue: Promise<void> = Promise.resolve();
  const cancelled = new Set<number>();
  let lastHandled = 0;

  const cancelledError = () => new FoliarkError("CANCELLED", "operation cancelled", { recoverable: true });
  const need = (): wasm.Engine => {
    if (!engine) throw new FoliarkError("PROTOCOL_ERROR", "request before init");
    return engine;
  };

  /** Drive a started task to its result; a cancel or failure drops the task, leaving no state behind. */
  async function drive(id: number, e: wasm.Engine, task: number): Promise<any> {
    try {
      for (;;) {
        if (cancelled.has(id)) throw cancelledError();
        const s = e.step(task, stepBudget);
        if (s.done) return s.result;
        host.post({ v: 1, id, progress: s.progress as Progress });
        await yieldToEventLoop();
      }
    } finally {
      e.cancel(task);
    }
  }

  async function run(id: number, op: OpName, doc: string | undefined, a: Args): Promise<Out> {
    if (op === "init") {
      await host.loadWasm(a.wasmUrl).catch((e) => {
        throw new FoliarkError("ASSET_LOAD_FAILED", e instanceof Error ? e.message : String(e));
      });
      engine?.free();
      engine = new wasm.Engine(a.limits, generation!);
      return { result: null };
    }
    const e = need();
    const d = doc!;
    switch (op) {
      case "open":
        return { result: e.open(new Uint8Array(a.bytes), a.password, a.allowRepaired) };
      case "info":
        return { result: e.info(d) };
      case "pages":
        return { result: e.pages(d) };
      case "hitTest":
        return { result: e.hitTest(d, a.page, a.point, a.tolerance) ?? null };
      case "select":
        return { result: e.select(d, a.page, a.from, a.to) ?? null };
      case "quadsFor":
        return { result: e.quadsFor(d, a.page, a.start, a.end) };
      case "close":
        e.close(d);
        return { result: null };
      case "registerFonts":
        return { result: e.registerFonts(a.fonts.map((f: Args) => ({ ...f, bytes: new Uint8Array(f.bytes) }))) };
      case "prepareParagraph":
        return { result: e.prepareParagraph(d, a.page, a.rects, a.font, a.editorFace) };
      case "export": {
        const bytes = ownBuffer(e.takeExport(d, a.planId));
        return { result: bytes, transfer: [bytes] };
      }
      case "capabilities":
        return { result: await drive(id, e, e.startCapabilities(d, a.deep)) };
      case "extractText":
        return { result: await drive(id, e, e.startExtractText(d, a.page, defined({ chars: a.chars, includeOcgHidden: a.includeOcgHidden }))) };
      case "search": {
        const { query, ...options } = a;
        return { result: await drive(id, e, e.startSearch(d, query, defined(options))) };
      }
      case "renderPage": {
        const { page, ...options } = a;
        const r = await drive(id, e, e.startRender(d, page, defined(options)));
        r.data = ownBuffer(r.data);
        return { result: r, transfer: [r.data] };
      }
      case "applyChanges":
        return { result: await drive(id, e, e.startApplyChanges(d, a.changes)) };
      case "prepareExport":
        return { result: await drive(id, e, e.startPrepareExport(d, a.mode, a.policy)) };
      default:
        throw new FoliarkError("INTERNAL", `op ${op} has no handler`);
    }
  }

  async function handle(raw: unknown): Promise<void> {
    const checked = checkRequest(raw, generation);
    if (!checked.ok) {
      // No readable id: nobody to answer, so tell the client this worker is unusable.
      if (checked.id === undefined) host.post({ v: 1, event: "fatal", gen: generation ?? 1, reason: checked.error.message });
      else host.post(errRes(checked.id, checked.error));
      return;
    }
    const { req } = checked;
    if (req.op === "init") generation = req.gen;
    try {
      if (cancelled.has(req.id)) throw cancelledError(); // queued ops cancel before they start
      const { result, transfer } = await run(req.id, req.op, req.doc, req.args as Args);
      const res = okRes(req.id, result);
      try {
        postTransfer({ postMessage: (m, o) => host.post(m, o.transfer as ArrayBuffer[]) }, res, transfer ?? []);
      } catch (e) {
        throw asFoliarkError(e);
      }
    } catch (err) {
      // A wasm trap leaves the instance unusable: the client resets the worker instead of getting an answer.
      if (err instanceof WebAssembly.RuntimeError) {
        host.post({ v: 1, event: "fatal", gen: generation, reason: "wasm trap" });
      } else {
        host.post(errRes(req.id, fromWasm(err)));
      }
    } finally {
      cancelled.delete(req.id);
      lastHandled = req.id;
    }
  }

  return {
    /** Entry point for every message from the main thread. */
    onMessage(raw: unknown): void {
      if (isCtl(raw)) {
        // A cancel for something already answered is stale and must not accumulate.
        if ("cancel" in raw && raw.cancel > lastHandled) cancelled.add(raw.cancel);
        return;
      }
      queue = queue.then(() => handle(raw));
    },
  };
}
