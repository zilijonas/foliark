#!/usr/bin/env node
// Regression budgets (NEW-65). Compare fresh SDK bench JSON against tools/bench/budgets.json.
//   node tools/bench/check-budgets.mjs FRESH.json [FRESH2.json ...] [--budgets FILE]
//   node tools/bench/check-budgets.mjs --derive FRESH.json ...      print a new budgets.json (the headroom rule below)
// FRESH files are outputs of `node tools/bench/sdk.mjs --extended` and `node tools/bench/scans-sdk.mjs`, one per browser run;
// the browser comes from each file's `browser` field. Exit 0 = every budget for the browsers present holds, 1 = a regression
// or a budgeted metric that was not measured (a missing number is never a pass), 2 = bad usage.
// Noise: pass the same run several times; the BEST median per metric is compared, so one slow neighbour does not fail it but
// a real slowdown (every run slow) does. Browsers without a fresh file are reported as skipped.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

// Headroom rule: max = measured * factor (+ an absolute floor for timings, because sub-10 ms medians move by milliseconds).
export const RULE = {
  timeMs: { factor: 1.5, floorMs: 3 },   // medians of a loaded shared machine moved up to ~10% between runs; 50% catches a real slowdown
  bytes: { factor: 1.05 },                // wasm and cold download: deterministic, 5% room for dependency bumps
  memory: { factor: 1.25 },               // wasm linear memory after all ops
};
const INIT = "init (worker + wasm instantiate)";
const NATIVE = "native export, N region redactions (apply+prepare+export)";
const RASTER = "raster export @2, N region redactions (apply+prepare+export)";
const sdk = (id, op, input, run, field = "ms") => ({ id, op, input, run, field, kind: field === "ms" ? "timeMs" : field === "bytes" ? "bytes" : "memory" });
const SCANS = ["jpeg-baseline-rgb", "jpeg-baseline-gray", "jpeg-progressive-rgb", "jpeg-progressive-gray", "jbig2", "ccitt-g4", "flate-gray"];
// What is budgeted, per browser: [id, op, input, run, field]. Everything else in the bench JSON is informational.
export const SELECTORS = [
  sdk("init cold", INIT, "-", "cold"), sdk("init cold download bytes", INIT, "-", "cold", "bytes"), sdk("init warm", INIT, "-", "warm"),
  sdk("text 30p open", "open", "native-30p", "warm"),
  sdk("text 30p first render", "first renderPage@1.35 right after open (incl. one-time document scan)", "native-30p", "warm"),
  sdk("text 30p render/page", "renderPage@1.35", "native-30p", "warm"),
  sdk("text 30p extract all", "extractText all pages", "native-30p", "warm"),
  sdk("text 30p open+extract+render all", "open+extract+render@1.35 all pages (HDDN LOAD_PDF equivalent)", "native-30p", "warm"),
  sdk("text 30p search", "search 'alpha'", "native-30p", "warm"),
  sdk("text 30p paragraph prepare", "paragraph edit: prepareParagraph", "native-30p", "warm"),
  sdk("text 30p paragraph edit apply", "paragraph edit: applyChanges (1 edit)", "native-30p", "warm"),
  sdk("text 30p native export", NATIVE, "native-30p", "warm"),
  sdk("text 30p raster export", RASTER, "native-30p", "warm"),
  sdk("text 30p wasm memory", "memory after ops", "native-30p", "warm", "wasmMemoryBytes"),
  sdk("image-heavy 6p first render", "first renderPage@1.35 right after open (incl. one-time document scan)", "image-heavy-6p", "warm"),
  sdk("image-heavy 6p render/page", "renderPage@1.35", "image-heavy-6p", "warm"),
  sdk("image-heavy 6p open+extract+render all", "open+extract+render@1.35 all pages (HDDN LOAD_PDF equivalent)", "image-heavy-6p", "warm"),
  ...SCANS.flatMap((s) => [
    sdk(`scan ${s} first@1.35`, "renderPage@1.35 first after open", `scan-${s}`),
    sdk(`scan ${s} repeat@1.35`, "renderPage@1.35 repeat", `scan-${s}`),
    sdk(`scan ${s} first@3`, "renderPage@3 first after open", `scan-${s}`),
  ]),
];

const num = (v) => typeof v === "number" && Number.isFinite(v);
const limit = (kind, measured) => {
  if (kind === "timeMs") return Math.ceil(measured * RULE.timeMs.factor + RULE.timeMs.floorMs);
  return Math.ceil(measured * RULE[kind].factor);
};
const load = (files) => files.map((f) => JSON.parse(readFileSync(f, "utf8")));
const browserOf = (j) => String(j.browser ?? "").split(" ")[0];
const find = (runs, b, s) => {
  const vals = runs.filter((j) => browserOf(j) === b).flatMap((j) => j.measurements ?? [])
    .filter((m) => m.op === s.op && m.input === s.input && (s.run === undefined || m.run === s.run) && num(m[s.field])).map((m) => m[s.field]);
  return vals.length ? Math.min(...vals) : undefined;
};
const wasm = (runs) => {
  const gz = runs.map((j) => j.wasmGzipBytes).filter(num), raw = runs.map((j) => j.assets?.wasmBytes).filter(num);
  return { gzip: gz.length ? Math.min(...gz) : undefined, raw: raw.length ? Math.min(...raw) : undefined };
};

export function derive(runs, meta = {}) {
  const browsers = [...new Set(runs.map(browserOf))].sort();
  const metrics = [];
  for (const b of browsers) for (const s of SELECTORS) {
    const measured = find(runs, b, s);
    if (measured !== undefined) metrics.push({ browser: b, id: s.id, op: s.op, input: s.input, ...(s.run && { run: s.run }), field: s.field, kind: s.kind, measured, max: limit(s.kind, measured) });
  }
  const w = wasm(runs);
  return { schema: 1, kind: "foliark-regression-budgets", rule: RULE, ...meta, wasm: { gzipBytes: { measured: w.gzip, max: limit("bytes", w.gzip) }, rawBytes: { measured: w.raw, max: limit("bytes", w.raw) } }, metrics };
}

export function check(budgets, runs) {
  const rows = [], browsers = new Set(runs.map(browserOf));
  const w = wasm(runs);
  for (const [id, key] of [["wasm gzip -9 bytes", "gzipBytes"], ["wasm raw bytes", "rawBytes"]]) {
    const got = key === "gzipBytes" ? w.gzip : w.raw;
    rows.push({ browser: "-", id, got, max: budgets.wasm[key].max, measured: budgets.wasm[key].measured });
  }
  for (const m of budgets.metrics) {
    if (!browsers.has(m.browser)) { rows.push({ browser: m.browser, id: m.id, skipped: true }); continue; }
    rows.push({ browser: m.browser, id: m.id, got: find(runs, m.browser, m), max: m.max, measured: m.measured });
  }
  for (const r of rows) r.status = r.skipped ? "skipped" : r.got === undefined ? "NOT MEASURED" : r.got > r.max ? "REGRESSION" : "ok";
  return rows;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const bi = args.indexOf("--budgets");
  const budgetsPath = bi >= 0 ? args.splice(bi, 2)[1] : fileURLToPath(new URL("budgets.json", import.meta.url));
  const doDerive = args.includes("--derive");
  const files = args.filter((a) => !a.startsWith("--"));
  if (!files.length) { console.error("usage: check-budgets.mjs [--derive] FRESH.json ... [--budgets FILE]"); process.exit(2); }
  const runs = load(files);
  if (doDerive) { console.log(JSON.stringify(derive(runs, { measuredWith: runs.map((j) => ({ engine: j.engine, browser: j.browser, wasmSha256: j.wasmSha256 ?? undefined })) }), null, 1)); process.exit(0); }
  const rows = check(JSON.parse(readFileSync(budgetsPath, "utf8")), runs);
  for (const r of rows) console.log(`${r.status.padEnd(12)} ${r.browser.padEnd(9)} ${r.id.padEnd(46)} ${r.skipped ? "" : `${r.got ?? "-"} / max ${r.max} (frozen at ${r.measured})`}`);
  const bad = rows.filter((r) => r.status === "REGRESSION" || r.status === "NOT MEASURED");
  console.log(`${rows.filter((r) => r.status === "ok").length} ok, ${rows.filter((r) => r.skipped).length} skipped, ${bad.length} failing`);
  process.exit(bad.length ? 1 : 0);
}
