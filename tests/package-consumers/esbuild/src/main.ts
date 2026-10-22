// esbuild: the wasm is imported with the `file` loader (hashed name, URL under publicPath); the worker is a second
// entry point (build.mjs) and is found next to this script.
import { createFoliark } from "@zilijonas/foliark";
import wasmUrl from "@zilijonas/foliark/wasm";
import { mount } from "./ui.js";

mount(() => createFoliark({ wasmUrl, workerUrl: new URL("./worker.js", import.meta.url) }));
