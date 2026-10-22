// esbuild: the wasm is imported with the `file` loader (hashed name, URL under publicPath); the worker is a second
// entry point (build.mjs) and is found next to this script.
import { createFoliark } from "@foliark/sdk";
import wasmUrl from "@foliark/sdk/wasm";
import { mount } from "./ui.js";

mount(() => createFoliark({ wasmUrl, workerUrl: new URL("./worker.js", import.meta.url) }));
