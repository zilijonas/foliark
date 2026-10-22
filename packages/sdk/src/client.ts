// Main-thread client (spec 1, 10): owns the worker, speaks the validated envelope of spec 10.4, keeps the
// registered fonts so a reset can re-send them, and hands out `DocumentHandle`s. Every way a worker is replaced
// (hang watchdog, error, fatal, out of memory, restart()) goes through the one `reset` routine (spec 10.2).
import { makeHandle, type DocumentHandle, type Link, type Signal } from "./document.js";
import type { DocumentInfo } from "./generated/DocumentInfo.js";
import type { FontMatchRule } from "./generated/FontMatchRule.js";
import type { FontSource } from "./generated/FontSource.js";
import type { Limits } from "./generated/Limits.js";
import type { OpName } from "./generated/OpName.js";
import type { RegisteredFont } from "./generated/RegisteredFont.js";
import { Lifecycle } from "./lifecycle.js";
import { checkRequest, Correlator, FoliarkError, postTransfer } from "./protocol.js";
import { DEFAULT_HANG_TIMEOUT_MS, failureReason, Watchdog } from "./worker-supervisor.js";

export type ResetReason = "hang" | "crash" | "oom" | "restart";

export interface FoliarkOptions {
  wasmUrl: string | URL;
  /** Exactly one of `workerUrl` and `worker`. */
  workerUrl?: string | URL;
  worker?: () => Worker;
  /** Reserved (spec 10.3). 0.1.0 ships no CMap pack and never fetches this URL; pages that need a predefined CMap report `requiredAssets` and render with a `CMAP_MISSING` warning. */
  cmapsUrl?: string | URL;
  limits?: Partial<Limits>;
  fontMatch?: FontMatchRule[];
  /** Default 10000 (spec 10.2). */
  hangTimeoutMs?: number;
  /** Called with a reason (never content) for every message that broke the protocol and was dropped or failed. */
  onProtocolViolation?: (reason: string) => void;
}

export interface OpenOptions extends Signal {
  /** User or owner password; not retained. */
  password?: string;
  /** Accept a file whose xref had to be rebuilt (false). */
  allowRepaired?: boolean;
  /** An `ArrayBuffer` is transferred (detached in the caller) unless `false`; a view is always copied. Default true. */
  transfer?: boolean;
}

export interface Foliark {
  /** +1 per worker (re)start. */
  readonly generation: number;
  open(bytes: ArrayBuffer | Uint8Array, o?: OpenOptions): Promise<DocumentHandle>;
  registerFonts(f: FontSource[], o?: Signal): Promise<RegisteredFont[]>;
  /** Hard reset: a fresh worker, pending calls reject `WORKER_RESET`, old handles are gone (spec 10.2). */
  restart(): Promise<void>;
  onReset(cb: (e: { generation: number; reason: ResetReason }) => void): () => void;
  /** Later calls reject `CLIENT_DISPOSED`. */
  dispose(): Promise<void>;
}

type Waiter = { resolve(v: any): void; reject(e: Error): void };

const copyOf = (b: ArrayBuffer | Uint8Array): Uint8Array => (b instanceof Uint8Array ? b.slice() : new Uint8Array(b.slice(0)));

export async function createFoliark(o: FoliarkOptions): Promise<Foliark> {
  if ((o.workerUrl === undefined) === (o.worker === undefined)) {
    throw new FoliarkError("INVALID_ARGUMENT", "pass exactly one of workerUrl and worker");
  }
  const hangTimeoutMs = o.hangTimeoutMs ?? DEFAULT_HANG_TIMEOUT_MS;
  if (!(Number.isFinite(hangTimeoutMs) && hangTimeoutMs > 0)) {
    throw new FoliarkError("INVALID_ARGUMENT", "hangTimeoutMs must be a positive number");
  }
  const corr = new Correlator<Waiter>();
  const violation = o.onProtocolViolation ?? ((reason) => console.warn(`foliark protocol violation: ${reason}`));
  const listeners = new Set<(e: { generation: number; reason: ResetReason }) => void>();
  /** Client-held bytes of every registered font, re-sent after a reset (spec 10.2). */
  const fonts: { bytes: Uint8Array; family: string; weight: number; style: string }[] = [];
  const lifecycle = new Lifecycle();
  let disposed = false;
  let worker!: Worker;
  // No message from the worker for `hangTimeoutMs` while a request is unanswered: it is stuck in one call.
  const watchdog = new Watchdog(hangTimeoutMs, () => {
    violation(`worker silent for ${hangTimeoutMs} ms`);
    reset("hang").catch(resetFailed);
  });
  const resetFailed = (e: unknown) => {
    if (!disposed) violation(`reset failed: ${e instanceof Error ? e.message : String(e)}`);
  };
  /** Terminal: no worker, no timer, every waiter settled. */
  const shutdown = () => {
    disposed = true;
    watchdog.stop();
    worker.terminate();
    const err = new FoliarkError("CLIENT_DISPOSED", "client disposed");
    for (const w of corr.reset()) w.reject(err);
    lifecycle.releaseAll();
    fonts.length = 0; // the client-held font bytes are the one big thing it keeps
    listeners.clear();
  };

  const call = <R>(op: OpName, args: object, c: { doc?: string; transfer?: ArrayBuffer[]; signal?: AbortSignal } = {}) =>
    new Promise<R>((resolve, reject) => {
      if (disposed) return reject(new FoliarkError("CLIENT_DISPOSED", "client disposed"));
      if (c.signal?.aborted) return reject(new FoliarkError("CANCELLED", "operation cancelled", { recoverable: true }));
      const msg = { v: 1 as const, id: 0, gen: corr.generation, op, ...(c.doc === undefined ? {} : { doc: c.doc }), args };
      const checked = checkRequest(msg, corr.generation);
      if (!checked.ok) return reject(checked.error);
      let onAbort: (() => void) | undefined;
      const settle = (f: () => void) => () => {
        if (onAbort) c.signal?.removeEventListener("abort", onAbort);
        f();
      };
      const id = corr.register(op, {
        resolve: (v) => settle(() => resolve(v))(),
        reject: (e) => settle(() => reject(e))(),
      });
      msg.id = id;
      watchdog.arm();
      // The worker answers a cancelled op itself (CANCELLED), so the correlator settles it like any other.
      if (c.signal) {
        onAbort = () => worker.postMessage({ v: 1, cancel: id });
        c.signal.addEventListener("abort", onAbort, { once: true });
      }
      try {
        postTransfer(worker, msg, c.transfer ?? []);
      } catch (e) {
        reject(e as Error);
      }
    });

  const fontArgs = () => ({ fonts: fonts.map((f) => ({ ...f, bytes: f.bytes.slice().buffer })) });

  /** Start a worker for the current generation: init, then the registered fonts, in order. */
  function spawn(first = false): Promise<unknown> {
    const w = (worker = o.worker ? o.worker() : new Worker(o.workerUrl!, { type: "module" }));
    w.onmessage = (e) => {
      if (w !== worker) return;
      const m = corr.receive(e.data);
      if (corr.size) watchdog.kick();
      else watchdog.stop();
      if (m.kind === "result") m.ctx.resolve(m.value);
      else if (m.kind === "failure") {
        if (m.error.code === "PROTOCOL_ERROR") violation(m.error.message);
        m.ctx.reject(m.error);
      } else if (m.kind === "dropped") violation(m.reason);
      else if (m.kind === "event" && m.event === "fatal") crash(m.reason ?? "worker fatal");
    };
    // A first worker that dies before init answered could not load its script (missing, offline): an asset failure (spec 10.3).
    // Later workers keep the crash path (their script was already fetched once).
    let failInit!: (e: Error) => void;
    const unloadable = new Promise<never>((_, reject) => (failInit = reject));
    let starting = first;
    w.onerror = (e) => {
      if (w !== worker) return;
      if (starting) failInit(new FoliarkError("ASSET_LOAD_FAILED", `worker script failed to load${e.message ? `: ${e.message}` : ""}`));
      else crash(`worker error: ${e.message}`, e.message);
    };
    const initCall = call("init", {
      wasmUrl: String(o.wasmUrl),
      cmapsUrl: o.cmapsUrl === undefined ? undefined : String(o.cmapsUrl),
      limits: o.limits,
      fontMatch: o.fontMatch,
    });
    const init = Promise.race([unloadable, initCall]).finally(() => (starting = false));
    const registered = fonts.length
      ? (() => {
          const args = fontArgs();
          return call("registerFonts", args, { transfer: args.fonts.map((f) => f.bytes) });
        })()
      : undefined;
    return Promise.all([init, registered]);
  }

  async function reset(reason: ResetReason): Promise<void> {
    if (disposed) return;
    watchdog.stop();
    worker.terminate();
    lifecycle.releaseAll(); // every handle of the dead generation is gone, whoever still holds it
    const err = new FoliarkError("WORKER_RESET", `worker reset (${reason})`, { recoverable: true });
    for (const w of corr.reset()) w.reject(err);
    const ready = spawn();
    for (const cb of [...listeners]) {
      try {
        cb({ generation: corr.generation, reason });
      } catch {
        // a listener's bug must not break the reset
      }
    }
    await ready;
  }
  const crash = (why: string, cause: unknown = why) => {
    violation(why);
    reset(failureReason(cause)).catch(resetFailed);
  };

  const link: Link = {
    get generation() {
      return corr.generation;
    },
    get disposed() {
      return disposed;
    },
    call,
    own: (release) => lifecycle.own(release),
  };

  try {
    await spawn(true);
  } catch (e) {
    // A client that never started owns no worker.
    shutdown();
    throw e;
  }

  return {
    get generation() {
      return corr.generation;
    },
    async open(bytes, opts) {
      const transferable = bytes instanceof ArrayBuffer && opts?.transfer !== false;
      const buf = transferable ? bytes : (copyOf(bytes).buffer as ArrayBuffer);
      const { docId, info } = await call<{ docId: string; info: DocumentInfo }>(
        "open",
        { bytes: buf, password: opts?.password, allowRepaired: opts?.allowRepaired },
        { transfer: [buf], signal: opts?.signal },
      );
      return makeHandle(link, docId, info);
    },
    async registerFonts(list, opts) {
      const kept = list.map((f) => ({ bytes: copyOf(f.bytes), family: f.family, weight: f.weight, style: f.style }));
      const args = { fonts: kept.map((f) => ({ ...f, bytes: f.bytes.slice().buffer })) };
      const registered = await call<RegisteredFont[]>("registerFonts", args, { transfer: args.fonts.map((f) => f.bytes), signal: opts?.signal });
      fonts.push(...kept);
      return registered;
    },
    restart: () => reset("restart"),
    onReset(cb) {
      listeners.add(cb);
      return () => void listeners.delete(cb);
    },
    async dispose() {
      if (!disposed) shutdown();
    },
  };
}
