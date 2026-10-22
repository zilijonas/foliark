import { createFoliark } from "./sdk/dist/index.js";
import { mount } from "./ui.js";

mount(() =>
  createFoliark({
    wasmUrl: new URL("./sdk/wasm/foliark_wasm_bg.wasm", import.meta.url),
    workerUrl: new URL("./sdk/dist/worker.js", import.meta.url),
  }),
);
