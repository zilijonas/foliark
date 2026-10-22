// Module worker entry: binds the worker core to `self` and fetches the wasm (spec 10.1, 10.3).
import init from "../wasm/foliark_wasm.js";
import { createWorkerCore } from "./worker-core.js";

const core = createWorkerCore({
  post: (message, transfer = []) => postMessage(message, { transfer }),
  loadWasm: (url) => init({ module_or_path: url }).then(() => undefined),
});
self.onmessage = (e: MessageEvent) => core.onMessage(e.data);
