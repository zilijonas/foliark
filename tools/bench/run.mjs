// Bench runner. Usage: node tools/bench/run.mjs [--smoke] [--browser chromium|webkit] [--out FILE]
// Full run writes docs/performance/baseline.json; --smoke (N=2, 2 fixtures) writes tools/bench/.cache/smoke.json.
// Run browser benches through the memory queue (see docs/performance/README.md).
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { ROOT, BENCH, machine } from "./lib.mjs";
import { runHddn } from "./hddn-worker.mjs";
import { runSdk } from "./sdk.mjs";

const arg = (k) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : undefined; };
const smoke = process.argv.includes("--smoke"), browserName = arg("--browser") ?? "chromium";
const out = arg("--out") ?? (smoke ? join(BENCH, ".cache/smoke.json") : join(ROOT, "docs/performance/baseline.json"));
const hddn = await runHddn({ smoke, browserName });
const sdk = await runSdk({ smoke, browserName });
const tag = (r, engine) => r.measurements.map((m) => ({ engine, ...m }));
const doc = {
  schema: 1, createdAt: new Date().toISOString(), mode: smoke ? "smoke" : "full", machine: machine(),
  browser: hddn.browser, engines: { hddn: hddn.engine, sdk: sdk.engine }, sdkNote: sdk.engineNote,
  inputs: hddn.inputs,
  measurements: [...tag(hddn, "hddn"), ...tag(sdk, "sdk")],
  assets: { hddn: hddn.assets, sdk: sdk.assets },
  unavailable: [...hddn.unavailable.map((u) => ({ engine: "hddn", ...u })), ...sdk.unavailable.map((u) => ({ engine: "sdk", ...u }))],
  notes: ["ms = median of n, p95 over the same samples; localhost, identity encoding (wire bytes = file bytes); headless", "GET_PAGE_PREVIEW and renderPage are main-thread round trips incl. postMessage and PNG/RGBA transfer; HDDN LOAD_PDF includes extraction and page previews"],
};
await mkdir(dirname(out), { recursive: true });
await writeFile(out, JSON.stringify(doc, null, 1) + "\n");
console.error(`wrote ${out} (${doc.measurements.length} measurements, ${doc.unavailable.length} unavailable)`);
