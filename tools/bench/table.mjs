// Markdown summary table from a comparison.json. Usage: node tools/bench/table.mjs docs/performance/comparison.json
import { readFileSync } from "node:fs";
const d = JSON.parse(readFileSync(process.argv[2], "utf8"));
const f = (x) => (x === undefined ? "n/a" : x >= 100 ? Math.round(x).toLocaleString("en") : String(x));
const mb = (b) => (b === undefined ? "n/a" : (b / 1e6).toFixed(2) + " MB");
for (const [name, run] of Object.entries(d.browsers)) {
  const get = (engine, op, input, runKind) => run.measurements.find((m) => m.engine === engine && m.op.startsWith(op) && (input === undefined || m.input === input) && (runKind === undefined || m.run === runKind));
  const row = (label, h, s, note = "") => `| ${label} | ${h ? `${f(h.ms)} (p95 ${f(h.p95)}, n=${h.n})` : "n/a"} | ${s ? `${f(s.ms)} (p95 ${f(s.p95)}, n=${s.n})` : "n/a"} | ${note} |`;
  const lines = [`### ${run.browser}`, "", "| op (ms, median) | HDDN | Foliark SDK | note |", "|---|---|---|---|"];
  const hi = get("hddn", "INIT", "-", "cold"), si = get("sdk", "init", "-", "cold");
  lines.push(row(`init cold (${mb(hi?.bytes)} vs ${mb(si?.bytes)} downloaded)`, hi, si));
  lines.push(row("init warm (assets cached)", get("hddn", "INIT", "-", "warm"), get("sdk", "init", "-", "warm")));
  for (const id of ["native-30p", "image-heavy-6p"]) {
    lines.push(row(`${id}: open + extract + render@1.35 every page`, get("hddn", "LOAD_PDF", id), get("sdk", "open+extract+render", id), "HDDN LOAD_PDF = parse + extract + page previews"));
    const hp = get("hddn", "GET_PAGE_PREVIEW", id), sp = get("sdk", "renderPage@1.35", id);
    lines.push(row(`${id}: render@1.35 per page`, hp, sp, "HDDN encodes PNG, SDK returns RGBA"));
    if (get("sdk", "first renderPage", id)) lines.push(row(`${id}: first render after open (SDK)`, undefined, get("sdk", "first renderPage", id), "includes one-time document scan"));
  }
  lines.push(row("native-30p: extract all pages", undefined, get("sdk", "extractText all", "native-30p"), "SDK only (no HDDN op without previews)"));
  lines.push(row("native-30p: search 'alpha' (SDK) / DETECT (HDDN)", get("hddn", "DETECT", "native-30p"), get("sdk", "search", "native-30p"), "NOT comparable: DETECT also runs PII, names, signatures"));
  lines.push(row("native-30p: paragraph prepare", get("hddn", "PREPARE_PARAGRAPH_EDIT", "native-30p"), get("sdk", "paragraph edit: prepare", "native-30p")));
  lines.push(row("native-30p: paragraph edit apply", get("hddn", "SET_TEXT_EDITS (same", "native-30p"), get("sdk", "paragraph edit: applyChanges", "native-30p"), "HDDN also regenerates previews"));
  const hn = get("hddn", "APPLY_REDACTIONS native (N", "native-30p"), sn = get("sdk", "native export", "native-30p");
  lines.push(row("native-30p: native export, 10 regions", hn, sn, `out ${f(hn?.outBytes)} B vs ${f(sn?.outBytes)} B; same words removed`));
  const hr = get("hddn", "APPLY_REDACTIONS flattened", "native-30p"), sr = get("sdk", "raster export", "native-30p");
  lines.push(row("native-30p: raster export @2, 10 regions", hr, sr, `out ${f(hr?.outBytes)} B vs ${f(sr?.outBytes)} B; different rasterizers`));
  const hm = run.measurements.find((m) => m.engine === "hddn" && m.op === "memory after ops" && m.input === "native-30p"), sm = run.measurements.find((m) => m.engine === "sdk" && m.op === "memory after ops" && m.input === "native-30p");
  lines.push(`| memory after all ops, native-30p | wasm ${mb(hm?.wasmMemoryBytes)}, browser RSS ${mb(hm?.rssBytes)} | wasm ${mb(sm?.wasmMemoryBytes)}, browser RSS ${mb(sm?.rssBytes)} | RSS = whole browser tree, not heap |`);
  console.log(lines.join("\n") + "\n");
}
