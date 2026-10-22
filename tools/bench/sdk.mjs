// Foliark SDK baseline through the current smoke API: init + open + renderPage(scale 1.35) per page + close.
// Uses the prebuilt packages/sdk/dist + packages/sdk/wasm (never builds Rust). Usage: node tools/bench/sdk.mjs [--smoke]
import { access, readFile, mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { execFileSync } from "node:child_process";
import { join } from "node:path";
import playwright from "playwright";
import { BENCH, FIXTURES, EXTENDED_FIXTURES, ROOT, gitSha, loadFixtures, serve, sha256, stat1, treeRssBytes, OUT_DIR, EDIT_LINES } from "./lib.mjs";

const SCALE = 1.35;
// extended: the full op set of docs/performance/comparison.json (extract, search, edit, native + raster export, memory).
export async function runSdk({ smoke = false, browserName = "chromium", extended = false } = {}) {
  const N = smoke ? { cold: 1, warm: 2, op: 2 } : { cold: 3, warm: 5, op: 5 };
  const wasmPath = join(ROOT, "packages/sdk/wasm/foliark_wasm_bg.wasm");
  try { await access(wasmPath); await access(join(ROOT, "packages/sdk/dist/index.js")); }
  catch { return { engine: `foliark@${gitSha()}`, measurements: [], inputs: [], unavailable: [{ op: "all SDK ops", reason: "packages/sdk/dist or packages/sdk/wasm not built" }] }; }
  const wasm = await readFile(wasmPath);
  const fixtures = await loadFixtures(smoke ? FIXTURES.filter((f) => f[0] === "native-small" || f[0] === "native-30p") : extended ? EXTENDED_FIXTURES : FIXTURES);
  const srv = await serve([{ prefix: "/__bench/", dir: BENCH }, { prefix: "/", dir: ROOT }]);
  const url = `${srv.origin}/__bench/sdk-page.html`;
  const measurements = [], unavailable = [], assets = { wasmBytes: wasm.length };
  const add = (op, input, run, samples, extra = {}) => measurements.push({ op, input, run, ...stat1(samples), ...extra });
  // like-for-like with HDDN LOAD_PDF (parse + extract + a preview per page): open + extract + render@1.35 every page
  const composite = async (page, f) => {
    const comp = [];
    for (let i = 0; i < Math.min(N.op, f.pages > 5 ? 3 : N.op); i++) {
      await page.evaluate(() => sdkClose()); const b64 = f.buf.toString("base64");
      const o = await page.evaluate(async (b) => { const x = await sdkOpen(b); let ms = x.ms; ms += (await sdkExtractAll()).ms; for (let p = 0; p < x.pages; p++) ms += (await sdkRender(p, 1.35)).ms; return ms; }, b64);
      comp.push(o);
    }
    add("open+extract+render@1.35 all pages (HDDN LOAD_PDF equivalent)", f.id, "warm", comp, { pages: f.pages });
  };
  const extendedOps = async (page, f) => {
    if (f.id === "image-heavy-6p") return composite(page, f);
    const big = f.id === "native-30p", heavy = Math.min(N.op, 3);
    const lines = await page.evaluate((n) => sdkLines(n), EDIT_LINES);
    const common = { pages: f.pages };
    const ex = [], words = [];
    for (let i = 0; i < N.op; i++) { const r = await page.evaluate(() => sdkExtractAll()); ex.push(r.ms); words.push(r.words); }
    add("extractText all pages", f.id, "warm", ex, { ...common, words: words[0] });
    await composite(page, f);
    const s = []; let hits = 0;
    for (let i = 0; i < N.op; i++) { const r = await page.evaluate(() => sdkSearch("alpha")); s.push(r.ms); hits = r.hits; }
    add("search 'alpha'", f.id, "warm", s, { ...common, hits });
    const fnt = await page.evaluate((u) => sdkRegisterFont(u), "/crates/foliark-text/tests/data/LiberationSans-Regular.ttf");
    add("registerFonts (Liberation Sans, the HDDN edit fallback)", "-", "warm", [fnt.ms], { bytes: fnt.bytes });
    const prep = [], app = [];
    for (let i = 0; i < heavy; i++) { const r = await page.evaluate((l) => sdkParaEdit(l), lines[0]); if (r.err) { unavailable.push({ op: "paragraph edit", input: f.id, reason: r.err }); break; } prep.push(r.prepareMs); app.push(r.applyMs); }
    if (prep.length) { add("paragraph edit: prepareParagraph", f.id, "warm", prep, common); add("paragraph edit: applyChanges (1 edit)", f.id, "warm", app, common); }
    await mkdir(OUT_DIR, { recursive: true });
    for (const [mode, label] of [["native", "native export, N region redactions (apply+prepare+export)"], ["raster", "raster export @2, N region redactions (apply+prepare+export)"]]) {
      const tot = [], parts = { applyMs: [], prepareMs: [], exportMs: [] }; let first;
      for (let i = 0; i < heavy; i++) {
        const r = await page.evaluate(([m, l]) => sdkExport(m, l), [mode, lines]);
        if (r.err) { unavailable.push({ op: label, input: f.id, reason: r.err }); break; }
        first ??= r; tot.push(r.applyMs + r.prepareMs + r.exportMs); for (const k in parts) parts[k].push(r[k]);
      }
      if (!first) continue;
      await writeFile(join(OUT_DIR, `sdk-${f.id}-${mode}.pdf`), Buffer.from(first.out, "base64"));
      add(label, f.id, "warm", tot, { ...common, regions: lines.length, outBytes: first.bytes, losses: first.losses, removed: first.removed, breakdownMedianMs: Object.fromEntries(Object.entries(parts).map(([k, v]) => [k, stat1(v).ms])) });
    }
    measurements.push({ op: "memory after ops", input: f.id, run: "warm", rssBytes: treeRssBytes(), ...(await page.evaluate(() => wasmMem())) });
    editLines[f.id] = lines;
  };
  const editLines = {};
  const dirs = []; let browserVersion = "?", warmCtx;
  const openPage = async (ctx) => { const p = await ctx.newPage(); p.on("pageerror", (e) => console.error("pageerror:", e.message)); await p.goto(url); await p.waitForFunction(() => window.ready); return p; };
  try {
    const cold = [];
    for (let i = 0; i < N.cold; i++) {
      const dir = await mkdtemp(join(tmpdir(), "sdk-bench-")); dirs.push(dir);
      const ctx = await playwright[browserName].launchPersistentContext(dir); browserVersion = ctx.browser()?.version() ?? browserVersion;
      const p = await openPage(ctx);
      srv.bytes.clear();
      cold.push(await p.evaluate(() => initSdk()));
      if (i === 0) assets.files = Object.fromEntries([...srv.bytes].filter(([f]) => f.startsWith("/packages/") || f === "/__bench/sdk-worker.js"));
      if (i === N.cold - 1) { warmCtx = ctx; await p.evaluate(() => sdkDispose()); await p.close(); } else await ctx.close();
    }
    assets.initBytes = Object.entries(assets.files).filter(([f]) => !f.includes("/__bench/") ).reduce((a, [, n]) => a + n, 0) + (assets.files["/__bench/sdk-worker.js"] ?? 0);
    add("init (worker + wasm instantiate)", "-", "cold", cold, { bytes: assets.initBytes });
    const warm = [];
    for (let i = 0; i < N.warm; i++) { const p = await openPage(warmCtx); warm.push(await p.evaluate(() => initSdk())); await p.evaluate(() => sdkDispose()); await p.close(); }
    add("init (worker + wasm instantiate)", "-", "warm", warm);
    const page = await openPage(warmCtx); await page.evaluate(() => initSdk());
    for (const f of fixtures) {
      const b64 = f.buf.toString("base64"), opens = [], closes = [], firsts = []; let pages;
      try {
        for (let i = 0; i < N.op; i++) { const o = await page.evaluate((b) => sdkOpen(b), b64); opens.push(o.ms); pages = o.pages; if (extended) firsts.push((await page.evaluate(({ s }) => sdkRender(0, s), { s: SCALE })).ms); if (i < N.op - 1) closes.push(await page.evaluate(() => sdkClose())); }
        const per = Array.from({ length: pages }, () => []);
        for (let r = 0; r < N.op; r++) for (let p = 0; p < pages; p++) per[p].push((await page.evaluate(({ p, s }) => sdkRender(p, s), { p, s: SCALE })).ms);
        add("open", f.id, "warm", opens, { pages });
        if (extended) add("first renderPage@1.35 right after open (incl. one-time document scan)", f.id, "warm", firsts, { pages });
        add("renderPage@1.35", f.id, "warm", per.flat(), { pages, perPageMedianMs: per.map((a) => stat1(a).ms) });
        if (f.id === "native-30p") measurements.push({ op: "rss-after-open+render", input: f.id, run: "warm", rssBytes: treeRssBytes() });
        if (extended && (f.id === "native-30p" || f.id === "native-small" || f.id === "image-heavy-6p")) await extendedOps(page, f);
        closes.push(await page.evaluate(() => sdkClose()));
        add("close", f.id, "warm", closes);
      } catch (e) { unavailable.push({ op: "open/renderPage", input: f.id, reason: String(e.message).split("\n")[0].slice(0, 200) }); await page.evaluate(() => sdkClose()).catch(() => {}); }
    }
    await page.evaluate(() => sdkDispose()); await warmCtx.close();
  } finally { srv.close(); for (const d of dirs) await rm(d, { recursive: true, force: true }); }
  if (!extended) for (const op of ["text extraction", "search", "text edit (SET_TEXT_EDITS equivalent)", "redaction apply", "export / save"]) unavailable.push({ op, reason: "not measured in the baseline run (see docs/performance/comparison.json)" });
  unavailable.push({ op: "JS heap", reason: "performance.measureUserAgentSpecificMemory not available in headless Chromium or WebKit; process-tree RSS recorded instead" });
  return { engine: `foliark@${gitSha()}`, engineNote: `prebuilt packages/sdk/{dist,wasm}; wasm sha256 ${sha256(wasm).slice(0, 16)} (${wasm.length} B); built from the checked-out tree`, browser: `${browserName} ${browserVersion}`, inputs: fixtures.map(({ id, sha256, pages, bytes, origin }) => ({ id, sha256, pages, bytes, origin })), measurements, unavailable, assets, editLines, wasmSha256: sha256(wasm), wasmGzipBytes: execFileSync("gzip", ["-9", "-n", "-c"], { input: wasm, maxBuffer: 1 << 28 }).length };
}
if (import.meta.url === `file://${process.argv[1]}`) console.log(JSON.stringify(await runSdk({ smoke: process.argv.includes("--smoke"), extended: process.argv.includes("--extended") }), null, 1));
