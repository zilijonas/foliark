// The worker's logic, separate from `self` so vitest can run it in-process over a real MessageChannel.
// One wasm `Engine` per worker, many documents, one request at a time in arrival order (spec 10.1).
// Long ops are Rust tasks driven step by step: between steps the loop yields to the event loop (so a
// cancel message can arrive) and posts progress. Every request is validated against spec 10.4.
import * as wasm from "../wasm/foliark_wasm.js";
import type { OpName } from "./generated/OpName.js";
import type { Progress } from "./generated/Progress.js";
import { isOutOfMemory } from "./worker-supervisor.js";
import {
  asFoliarkError, checkRequest, errRes, FoliarkError, isCtl, isErrorCode, okRes, postTransfer,
} from "./protocol.js";

/** Spec 10.1: work units per step. */
export const STEP_BUDGET = 100_000;

export interface WorkerHost {
  post(message: unknown, transfer?: ArrayBuffer[]): void;
  /** Instantiates the wasm module (fetch in the real worker, bytes in tests). */
  loadWasm(url: string): Promise<void>;
  /** Where the CMap pack lives when `init` names no `cmapsUrl`: the `cmaps/` folder next to this package's `dist/`. */
  cmapsUrl?: string;
  /** Bytes of one pack file (default `fetch`; tests read the folder). Rejects when it cannot be had. */
  fetchCMap?(url: string): Promise<Uint8Array>;
}

/** One pack file over HTTP: same origin by default, no credentials, nothing but the file name leaves the device. */
async function fetchPackFile(url: string): Promise<Uint8Array> {
  const res = await fetch(url, { credentials: "omit", cache: "force-cache" });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return new Uint8Array(await res.arrayBuffer());
}

const CMAP_NAME = /^[A-Za-z0-9][A-Za-z0-9-]{0,63}$/;
const USECMAP = /\/([A-Za-z0-9-]+)\s+usecmap/g;

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
  const cmaps = new Set<string>(); // registered in this engine
  let lastHandled = 0;
  let cmapsUrl: string | undefined;

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

  /**
   * Spec 10.3: fetch the pack files a document lists in `requiredAssets` (and the CMaps they `usecmap`), base first,
   * and register them. The names come from the PDF, so only a plain predefined-looking name ever becomes a URL.
   */
  async function loadCMaps(e: wasm.Engine, assets: string[], base: string, fetchCMap: (url: string) => Promise<Uint8Array>) {
    const dir = base.endsWith("/") ? base : `${base}/`;
    const failed = (name: string, why: string) =>
      new FoliarkError("ASSET_LOAD_FAILED", `CMap pack file cmap:${name} ${why}`, { details: { feature: `cmap:${name}` } });
    const load = async (name: string, chain: string[]): Promise<void> => {
      if (cmaps.has(name)) return;
      if (!CMAP_NAME.test(name) || chain.includes(name) || chain.length > 4) throw failed(name, "is not a loadable CMap name");
      const url = new URL(`${name}.cmap`, dir);
      let bytes: Uint8Array;
      try {
        bytes = await fetchCMap(url.href);
      } catch (err) {
        throw failed(name, `could not be loaded from ${url.href} (${err instanceof Error ? err.message : String(err)}); set cmapsUrl to a folder that serves the pack`);
      }
      for (const [, baseName] of new TextDecoder("latin1").decode(bytes.subarray(0, 4096)).matchAll(USECMAP)) {
        await load(baseName, [...chain, name]);
      }
      try {
        e.registerCMap(name, bytes);
      } catch {
        throw failed(name, "is corrupt or incomplete; reinstall the pack from the same @zilijonas/foliark version");
      }
      cmaps.add(name);
    };
    for (const a of assets) if (a.startsWith("cmap:")) await load(a.slice(5), []);
  }

  async function run(id: number, op: OpName, doc: string | undefined, a: Args): Promise<Out> {
    if (op === "init") {
      await host.loadWasm(a.wasmUrl).catch((e) => {
        throw new FoliarkError("ASSET_LOAD_FAILED", e instanceof Error ? e.message : String(e));
      });
      engine?.free();
      engine = new wasm.Engine(a.limits, generation!, a.fontMatch);
      cmaps.clear();
      cmapsUrl = a.cmapsUrl ?? host.cmapsUrl;
      return { result: null };
    }
    const e = need();
    const d = doc!;
    switch (op) {
      case "open": {
        const opened = e.open(new Uint8Array(a.bytes), a.password, a.allowRepaired);
        const needed = (opened.info.requiredAssets as string[]).filter((n) => n.startsWith("cmap:") && !cmaps.has(n.slice(5)));
        if (needed.length === 0) return { result: opened };
        try {
          if (!cmapsUrl) throw new FoliarkError("ASSET_LOAD_FAILED", "no CMap pack location: pass cmapsUrl", { details: { feature: needed[0] } });
          await loadCMaps(e, needed, cmapsUrl, host.fetchCMap ?? fetchPackFile);
        } catch (err) {
          e.close(opened.docId); // a document that cannot be read faithfully is not left open
          throw err;
        }
        return { result: opened };
      }
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
      case "prepareParagraph": {
        const r = e.prepareParagraph(d, a.page, a.rects, a.font, a.editorFace);
        if (!r.editorFace) return { result: r };
        r.editorFace.bytes = ownBuffer(r.editorFace.bytes);
        return { result: r, transfer: [r.editorFace.bytes] };
      }
      case "layoutEdit":
        return { result: e.layoutEdit(d, a.page, a.rects, a.text, a.font, a.glyphs) };
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
      if (!(err instanceof FoliarkError) && isOutOfMemory(err)) {
        // The failing op is answered; the instance may be half-grown, so the client replaces the worker (spec 9).
        host.post(errRes(req.id, new FoliarkError("OUT_OF_MEMORY", "out of memory", { recoverable: false })));
        host.post({ v: 1, event: "fatal", gen: generation, reason: "out of memory" });
      } else if (err instanceof WebAssembly.RuntimeError) {
        // A wasm trap leaves the instance unusable: the client resets the worker instead of getting an answer.
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
