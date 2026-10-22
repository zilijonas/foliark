// Module worker: owns the wasm instance and answers one request at a time, in order (spec 10.1).
// Every request is validated against the envelope of spec 10.4; smoke ops only until NEW-27 wires the rest.
import init, * as wasm from "../wasm/foliark_wasm.js";
import type { OpName } from "./generated/OpName.js";
import { asFoliarkError, checkRequest, errRes, FoliarkError, okRes, postTransfer } from "./protocol.js";

let generation: number | undefined;
let queue: Promise<void> = Promise.resolve();

/** The wasm facade still throws bare strings; NEW-27 replaces this with codes from `foliark-wasm`. */
function fromWasm(e: unknown): FoliarkError {
  const message = e instanceof Error ? e.message : String(e);
  if (/out of range|must be finite/.test(message)) return new FoliarkError("INVALID_ARGUMENT", message);
  if (/exceeds/.test(message)) return new FoliarkError("LIMIT_EXCEEDED", message);
  if (/unknown document handle/.test(message)) return new FoliarkError("DOCUMENT_GONE", message);
  if (/cannot open PDF/.test(message)) return new FoliarkError("MALFORMED_PDF", message);
  return asFoliarkError(e);
}

type Args = Record<string, any>;
async function run(op: OpName, doc: string | undefined, a: Args): Promise<{ result: unknown; transfer?: ArrayBuffer[] }> {
  const id = Number(doc);
  switch (op) {
    case "init":
      await init({ module_or_path: a.wasmUrl }).catch((e) => {
        throw new FoliarkError("ASSET_LOAD_FAILED", e instanceof Error ? e.message : String(e));
      });
      return { result: null };
    case "open": {
      const handle = wasm.open(new Uint8Array(a.bytes), a.password);
      return { result: { docId: String(handle), pageCount: wasm.pageCount(handle) } };
    }
    case "pages":
      return { result: Array.from({ length: wasm.pageCount(id) }, (_, i) => wasm.pageGeometry(id, i)) };
    case "renderPage": {
      const r = wasm.renderPage(id, a.page, a.scale ?? 1);
      const data = r.takeData().buffer as ArrayBuffer;
      const scale = a.scale ?? 1;
      const out = { width: r.width, height: r.height, scale, clip: { x: 0, y: 0, width: r.width / scale, height: r.height / scale }, complete: true, warnings: [], data };
      r.free();
      return { result: out, transfer: [data] };
    }
    case "close":
      wasm.close(id);
      return { result: null };
    default:
      throw new FoliarkError("INTERNAL", `op ${op} is not implemented yet`);
  }
}

async function handle(raw: unknown): Promise<void> {
  const checked = checkRequest(raw, generation);
  if (!checked.ok) {
    // No readable id: nobody to answer, so tell the client this worker is unusable.
    if (checked.id === undefined) postMessage({ v: 1, event: "fatal", gen: generation ?? 1, reason: checked.error.message });
    else postMessage(errRes(checked.id, checked.error));
    return;
  }
  const { req } = checked;
  if (req.op === "init") generation = req.gen;
  else if (generation === undefined) {
    postMessage(errRes(req.id, new FoliarkError("PROTOCOL_ERROR", "request before init")));
    return;
  }
  try {
    const { result, transfer } = await run(req.op, req.doc, req.args as Args);
    postTransfer(self, okRes(req.id, result), transfer ?? []);
  } catch (err) {
    postMessage(errRes(req.id, fromWasm(err)));
  }
}

self.onmessage = (e: MessageEvent) => {
  queue = queue.then(() => handle(e.data));
};
