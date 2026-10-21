// Thin main-thread client over the worker. Skeleton: no supervisor, validation or typed errors yet.
import type { PageGeometry } from "./generated/PageGeometry.js";

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
  open(bytes: Uint8Array, o?: { password?: string }): Promise<DocumentHandle>;
  dispose(): void;
}

export async function createFoliark(o: { wasmUrl: string | URL; workerUrl: string | URL }): Promise<Foliark> {
  const worker = new Worker(o.workerUrl, { type: "module" });
  let next = 0;
  const pending = new Map<number, { resolve(v: any): void; reject(e: Error): void }>();
  worker.onmessage = (e) => {
    const p = pending.get(e.data.id);
    if (!p) return;
    pending.delete(e.data.id);
    if (e.data.ok) p.resolve(e.data.result);
    else p.reject(new Error(e.data.error));
  };
  worker.onerror = (e) => {
    for (const p of pending.values()) p.reject(new Error(`worker error: ${e.message}`));
    pending.clear();
  };
  const call = (msg: object, transfer: Transferable[] = []) =>
    new Promise<any>((resolve, reject) => {
      const id = ++next;
      pending.set(id, { resolve, reject });
      worker.postMessage({ id, ...msg }, { transfer });
    });

  await call({ op: "init", wasmUrl: String(o.wasmUrl) });

  return {
    async open(bytes, opts) {
      const copy = bytes.slice();
      const doc: number = await call({ op: "open", bytes: copy, password: opts?.password }, [copy.buffer]);
      const pageCount: number = await call({ op: "pageCount", doc });
      return {
        pageCount,
        pageGeometry: (page) => call({ op: "pageGeometry", doc, page }),
        async renderPage(page, ro) {
          const r = await call({ op: "renderPage", doc, page, scale: ro?.scale ?? 1 });
          return { width: r.width, height: r.height, data: new Uint8ClampedArray(r.data.buffer) };
        },
        close: () => call({ op: "close", doc }),
      };
    },
    dispose: () => worker.terminate(),
  };
}
