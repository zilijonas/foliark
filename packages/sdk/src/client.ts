// Thin main-thread client over the worker, speaking the validated envelope of spec 10.4.
// Smoke ops only (open, pages, renderPage, close); the rest of the API, supervisor and restart arrive with NEW-27/29.
import type { OpName } from "./generated/OpName.js";
import type { PageGeometry } from "./generated/PageGeometry.js";
import { checkRequest, Correlator, FoliarkError, postTransfer } from "./protocol.js";

export interface RenderedPage {
  width: number;
  height: number;
  data: Uint8ClampedArray; // RGBA8 straight alpha
}

export interface DocumentHandle {
  readonly pageCount: number;
  pageGeometry(page: number): Promise<PageGeometry>;
  renderPage(page: number, o?: { scale?: number }): Promise<RenderedPage>;
  close(): Promise<void>;
}

export interface Foliark {
  /** An `ArrayBuffer` is transferred (detached in the caller) unless `transfer: false`; a view is always copied. */
  open(bytes: ArrayBuffer | Uint8Array, o?: { password?: string; transfer?: boolean }): Promise<DocumentHandle>;
  dispose(): void;
}

export interface FoliarkOptions {
  wasmUrl: string | URL;
  workerUrl: string | URL;
  /** Called with a reason (never content) for every message that broke the protocol and was dropped or failed. */
  onProtocolViolation?: (reason: string) => void;
}

type Waiter = { resolve(v: any): void; reject(e: Error): void };

export async function createFoliark(o: FoliarkOptions): Promise<Foliark> {
  const worker = new Worker(o.workerUrl, { type: "module" });
  const corr = new Correlator<Waiter>();
  const violation = o.onProtocolViolation ?? ((reason) => console.warn(`foliark protocol violation: ${reason}`));
  let dead: FoliarkError | undefined;

  const die = (e: FoliarkError) => {
    dead ??= e;
    worker.terminate();
    for (const w of corr.reset()) w.reject(e);
  };
  worker.onmessage = (e) => {
    const m = corr.receive(e.data);
    if (m.kind === "result") m.ctx.resolve(m.value);
    else if (m.kind === "failure") {
      if (m.error.code === "PROTOCOL_ERROR") violation(m.error.message);
      m.ctx.reject(m.error);
    } else if (m.kind === "dropped") violation(m.reason);
    else if (m.kind === "event" && m.event === "fatal") die(new FoliarkError("WORKER_RESET", m.reason ?? "worker fatal", { recoverable: true }));
  };
  worker.onerror = (e) => die(new FoliarkError("WORKER_RESET", `worker error: ${e.message}`, { recoverable: true }));

  const call = <R>(op: OpName, args: object, doc?: string, transfer: ArrayBuffer[] = []) =>
    new Promise<R>((resolve, reject) => {
      if (dead) return reject(dead);
      const msg = { v: 1 as const, id: 0, gen: corr.generation, op, ...(doc === undefined ? {} : { doc }), args };
      const checked = checkRequest(msg, corr.generation);
      if (!checked.ok) return reject(checked.error);
      msg.id = corr.register(op, { resolve, reject });
      try {
        postTransfer(worker, msg, transfer);
      } catch (e) {
        reject(e as Error);
      }
    });

  await call("init", { wasmUrl: String(o.wasmUrl) });

  return {
    async open(bytes, opts) {
      const buf = bytes instanceof ArrayBuffer && opts?.transfer !== false ? bytes : (bytes instanceof ArrayBuffer ? bytes.slice(0) : bytes.slice().buffer);
      const { docId, pageCount } = await call<{ docId: string; pageCount: number }>("open", { bytes: buf, password: opts?.password }, undefined, [buf]);
      let pages: Promise<PageGeometry[]> | undefined;
      return {
        pageCount,
        async pageGeometry(page) {
          const g = (await (pages ??= call<PageGeometry[]>("pages", {}, docId)))[page];
          if (!g) throw new FoliarkError("INVALID_ARGUMENT", `page ${page} out of range (count ${pageCount})`);
          return g;
        },
        async renderPage(page, ro) {
          const r = await call<{ width: number; height: number; data: ArrayBuffer }>("renderPage", { page, scale: ro?.scale ?? 1 }, docId);
          return { width: r.width, height: r.height, data: new Uint8ClampedArray(r.data) };
        },
        close: () => call<null>("close", {}, docId).then(() => undefined),
      };
    },
    dispose: () => die(new FoliarkError("CLIENT_DISPOSED", "client disposed")),
  };
}
