// Module worker: owns the wasm instance and answers one request at a time, in order.
// Skeleton protocol; the validated envelope of spec 10.4 lands with the protocol task.
import init, * as wasm from "../wasm/foliark_wasm.js";

type Msg = { id: number; op: string; [k: string]: any };

let ready: Promise<unknown> | undefined;
let queue: Promise<void> = Promise.resolve();

function handle(m: Msg): { result: unknown; transfer?: Transferable[] } {
  switch (m.op) {
    case "open":
      return { result: wasm.open(m.bytes, m.password) };
    case "pageCount":
      return { result: wasm.pageCount(m.doc) };
    case "pageGeometry":
      return { result: wasm.pageGeometry(m.doc, m.page) };
    case "renderPage": {
      const r = wasm.renderPage(m.doc, m.page, m.scale);
      const data = r.data;
      const out = { width: r.width, height: r.height, data };
      r.free();
      return { result: out, transfer: [data.buffer] };
    }
    case "close":
      wasm.close(m.doc);
      return { result: null };
    default:
      throw new Error(`unknown op ${m.op}`);
  }
}

self.onmessage = (e: MessageEvent<Msg>) => {
  const m = e.data;
  queue = queue.then(async () => {
    try {
      if (m.op === "init") {
        ready = init({ module_or_path: m.wasmUrl });
        await ready;
        postMessage({ id: m.id, ok: true, result: null });
        return;
      }
      await ready;
      const { result, transfer } = handle(m);
      postMessage({ id: m.id, ok: true, result }, { transfer });
    } catch (err) {
      postMessage({ id: m.id, ok: false, error: err instanceof Error ? err.message : String(err) });
    }
  });
};
