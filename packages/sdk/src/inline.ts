// In-worker entry (`@foliark/sdk/inline`): the same `Foliark` client and DTOs, but the engine runs in the CALLING
// thread instead of a nested worker. For hosts that already own a dedicated worker with its own generation and
// recovery (HDDN): they keep their protocol and state machine and import this inside that worker. Same worker
// core and the same validated envelope as the module worker, over a real MessageChannel, so buffers still transfer.
import init from "../wasm/foliark_wasm.js";
import { createFoliark, type Foliark, type FoliarkOptions } from "./client.js";
import { createWorkerCore } from "./worker-core.js";

export interface InlineOptions extends Omit<FoliarkOptions, "wasmUrl" | "workerUrl" | "worker" | "hangTimeoutMs"> {
  /** Where to fetch the wasm (the `@foliark/sdk/wasm` URL). Not needed when `wasm` is given. */
  wasmUrl?: string | URL;
  /** Already-fetched bytes or a compiled module (tests, prefetched or cached assets): nothing is fetched. */
  wasm?: BufferSource | WebAssembly.Module;
}

/** `setTimeout` caps at 2^31-1 ms; a same-thread engine cannot be watched from outside, its host supervises hangs. */
const NO_HANG_TIMER = 2 ** 31 - 1;

/**
 * Like `createFoliark` without a worker. Hang supervision belongs to the host: a stuck wasm call blocks the host
 * worker itself. After any `onReset` the host must treat its worker as dead and replace it, because a wasm trap or
 * out-of-memory cannot be healed inside the thread that owns the instance.
 */
export function createInlineFoliark(o: InlineOptions): Promise<Foliark> {
  const { wasm, wasmUrl, ...rest } = o;
  return createFoliark({
    ...rest,
    wasmUrl: wasmUrl ?? "inline:",
    hangTimeoutMs: NO_HANG_TIMER,
    worker: () => {
      const { port1, port2 } = new MessageChannel();
      const core = createWorkerCore({
        post: (m, transfer = []) => port2.postMessage(m, { transfer }),
        loadWasm: (url) => init({ module_or_path: wasm ?? url }).then(() => undefined),
      });
      port2.onmessage = (e) => core.onMessage(e.data);
      return Object.assign(port1, { terminate: () => (port1.close(), port2.close()) }) as unknown as Worker;
    },
  });
}
