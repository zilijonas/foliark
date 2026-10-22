// SDK vs HDDN full-op comparison -> docs/performance/comparison.json. Usage: node tools/bench/compare.mjs [--smoke] [--browsers chromium,webkit] [--out FILE]
// Run through the memory queue (docs/performance/README.md). Needs packages/sdk/{dist,wasm} built and the HDDN worktree built (HDDN_DIST).
import { execFileSync, spawnSync } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { ROOT, BENCH, OUT_DIR, HDDN_DIST, machine, gitSha } from "./lib.mjs";
import { runHddn } from "./hddn-worker.mjs";
import { runSdk } from "./sdk.mjs";

const arg = (k) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : undefined; };
const smoke = process.argv.includes("--smoke");
const browsers = (arg("--browsers") ?? "chromium,webkit").split(",");
const out = arg("--out") ?? (smoke ? join(BENCH, ".cache/comparison-smoke.json") : join(ROOT, "docs/performance/comparison.json"));
const dirty = spawnSync("git", ["-C", ROOT, "status", "--porcelain", "--", "crates", "packages/sdk/src", "Cargo.toml", "Cargo.lock", ".cargo", "vendor"], { encoding: "utf8" }).stdout.trim().length > 0;

const INPUT = { "native-small": join(ROOT, "tests/fixtures/hddn/metadata-xmp.pdf"), "native-30p": join(BENCH, ".cache/native-30p.pdf") };
const verify = async (sdk) => {
  const spec = [];
  for (const [id, lines] of Object.entries(sdk.editLines)) for (const [engine, mode] of [["sdk", "native"], ["sdk", "raster"], ["hddn", "native"], ["hddn", "flattened"]]) {
    spec.push({ file: join(OUT_DIR, `${engine}-${id}-${mode}.pdf`), input: INPUT[id], rects: lines.map((l) => [l.x, l.y, l.x + l.width, l.y + l.height]) });
  }
  const specFile = join(BENCH, ".cache/verify-spec.json");
  await writeFile(specFile, JSON.stringify(spec));
  return JSON.parse(execFileSync("python3", [join(BENCH, "verify-outputs.py"), specFile], { encoding: "utf8" }));
};

const runs = {}; let hddnRes, sdkRes;
for (const b of browsers) {
  sdkRes = await runSdk({ smoke, browserName: b, extended: true });
  hddnRes = await runHddn({ smoke, browserName: b, extended: true, lines: sdkRes.editLines });
  runs[b] = {
    browser: sdkRes.browser,
    measurements: [...hddnRes.measurements.map((m) => ({ engine: "hddn", ...m })), ...sdkRes.measurements.map((m) => ({ engine: "sdk", ...m }))],
    assets: { hddn: hddnRes.assets, sdk: sdkRes.assets },
    unavailable: [...hddnRes.unavailable.map((u) => ({ engine: "hddn", ...u })), ...sdkRes.unavailable.map((u) => ({ engine: "sdk", ...u }))],
    verifiedOutputs: await verify(sdkRes),
  };
  delete runs[b].assets.sdk.files; // per-file bytes kept only in the first run below
  runs[b].assets.sdkFiles = sdkRes.assets.files;
}
const wasm = await readFile(join(ROOT, "packages/sdk/wasm/foliark_wasm_bg.wasm"));
const doc = {
  schema: 1, kind: "foliark-vs-hddn-comparison", createdAt: new Date().toISOString(), mode: smoke ? "smoke" : "full",
  command: `node tools/bench/compare.mjs${smoke ? " --smoke" : ""} --browsers ${browsers.join(",")}   (through ~/ai/bin/memory-queue run --need-gb 2)`,
  machine: machine(),
  engines: {
    hddn: { rev: "2fe0f9a", engine: "Pyodide + PyMuPDF (wasm), pdf-lib flatten", workerSha256: hddnRes.assets.workerSha256, distDir: HDDN_DIST.replace(/^.*\/\.worktrees\//, ".worktrees/") },
    sdk: { rev: gitSha(), dirtySources: dirty, wasmSha256: sdkRes.wasmSha256, wasmBytes: wasm.length, wasmGzip9Bytes: sdkRes.wasmGzipBytes },
  },
  inputs: hddnRes.inputs,
  editLines: sdkRes.editLines,
  browsers: runs,
  quality: [
    { op: "native export", comparable: "yes for the measured output", hddn: "PyMuPDF true redaction; Info metadata removed; refuses native export for forms, invisible text, optional content", sdk: "strict native writer (unknown operators dropped, objects re-emitted, sanitize report, fail-closed on undecidable content)", note: "PyMuPDF re-read of both outputs (verifiedOutputs): the same words are gone from the same regions, the same other words remain, Info metadata gone, 0 embedded files. Structural/hidden-content sanitization is not measured here (SDK: strict_native suite; HDDN: no equivalent proof), so the SDK may do more work per byte." },
    { op: "raster export @2", comparable: "partial", hddn: "PyMuPDF render_page_png + pdf-lib flatten (PNG pages)", sdk: "hayro raster, redactions burned into pixels, losses disclosed per spec 7, PDF re-written by Foliark", note: "different rasterizers and image codecs; output sizes differ for that reason" },
    { op: "extract", comparable: "no", hddn: "LOAD_PDF = parse + span extraction + OCR-lane decision + page preview per page", sdk: "extractText = words, styles, clusters-optional", note: "compare HDDN LOAD_PDF only with the SDK open+extract+render composite" },
    { op: "search", comparable: "no", hddn: "no plain search op; DETECT also runs PII rules, name datasets and signature detection", sdk: "search(query) literal text search", note: "shown for scale only" },
    { op: "paragraph edit", comparable: "yes", hddn: "PREPARE_PARAGRAPH_EDIT + SET_TEXT_EDITS (re-renders affected page previews)", sdk: "prepareParagraph + applyChanges (layout + revision commit; no preview render)", note: "same text, same line, Liberation Sans fallback for both; HDDN set-edits includes preview regeneration" },
    { op: "render", comparable: "yes", hddn: "PyMuPDF get_pixmap @1.35 -> PNG encode", sdk: "hayro @1.35 -> RGBA8 transfer (no PNG encode)", note: "HDDN includes PNG encoding, the SDK returns raw pixels; SDK wins partly by skipping PNG" },
  ],
  notes: ["ms = median of n, p95 over the same samples; localhost, headless, identity encoding (wire bytes = file bytes)", "hddn and sdk inputs are byte-identical files (sha256 in inputs)", "unavailable lists what could not be measured and why"],
};
await mkdir(dirname(out), { recursive: true });
await writeFile(out, JSON.stringify(doc, null, 1) + "\n");
console.error(`wrote ${out}`);
