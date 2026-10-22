// Vite (the HDDN case): the package's asset entries are imported as URLs, so the bundler hashes and relocates them under
// `base`, and the same two URLs can go into an offline precache list.
import { createFoliark } from "@foliark/sdk";
import wasmUrl from "@foliark/sdk/wasm?url";
import workerUrl from "@foliark/sdk/worker?worker&url";
import { mount } from "./ui.js";

mount(() => createFoliark({ wasmUrl, workerUrl }));
