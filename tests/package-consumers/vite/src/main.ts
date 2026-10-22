// Vite (the HDDN case): the package's asset entries are imported as URLs, so the bundler hashes and relocates them under
// `base`, and the same two URLs can go into an offline precache list.
import { createFoliark } from "@zilijonas/foliark";
import wasmUrl from "@zilijonas/foliark/wasm?url";
import workerUrl from "@zilijonas/foliark/worker?worker&url";
import { mount } from "./ui.js";

mount(() => createFoliark({ wasmUrl, workerUrl }));
