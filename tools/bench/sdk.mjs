// Foliark SDK baseline through the current smoke API: init + open + renderPage(scale 1.35) per page + close.
// Uses the prebuilt packages/sdk/dist + packages/sdk/wasm (never builds Rust). Usage: node tools/bench/sdk.mjs [--smoke]
import { access, readFile, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import playwright from "playwright";
import { BENCH, FIXTURES, ROOT, gitSha, loadFixtures, serve, sha256, stat1, treeRssBytes } from "./lib.mjs";

const SCALE = 1.35;
export async function runSdk({ smoke = false, browserName = "chromium" } = {}) {
  const N = smoke ? { cold: 1, warm: 2, op: 2 } : { cold: 3, warm: 5, op: 5 };
  const wasmPath = join(ROOT, "packages/sdk/wasm/foliark_wasm_bg.wasm");
  try { await access(wasmPath); await access(join(ROOT, "packages/sdk/dist/index.js")); }
  catch { return { engine: `foliark@${gitSha()}`, measurements: [], inputs: [], unavailable: [{ op: "all SDK ops", reason: "packages/sdk/dist or packages/sdk/wasm not built" }] }; }
  const wasm = await readFile(wasmPath);
  const fixtures = await loadFixtures(smoke ? FIXTURES.filter((f) => f[0] === "native-small" || f[0] === "native-30p") : FIXTURES);
  const srv = await serve([{ prefix: "/__bench/", dir: BENCH }, { prefix: "/", dir: ROOT }]);
  const url = `${srv.origin}/__bench/sdk-page.html`;
  const measurements = [], unavailable = [];
  const add = (op, input, run, samples, extra = {}) => measurements.push({ op, input, run, ...stat1(samples), ...extra });
  const dirs = []; let browserVersion = "?", warmCtx;
  const openPage = async (ctx) => { const p = await ctx.newPage(); p.on("pageerror", (e) => console.error("pageerror:", e.message)); await p.goto(url); await p.waitForFunction(() => window.ready); return p; };
  try {
    const cold = [];
    for (let i = 0; i < N.cold; i++) {
      const dir = await mkdtemp(join(tmpdir(), "sdk-bench-")); dirs.push(dir);
      const ctx = await playwright[browserName].launchPersistentContext(dir); browserVersion = ctx.browser()?.version() ?? browserVersion;
      const p = await openPage(ctx);
      cold.push(await p.evaluate(() => initSdk()));
      if (i === N.cold - 1) { warmCtx = ctx; await p.evaluate(() => sdkDispose()); await p.close(); } else await ctx.close();
    }
    add("init (worker + wasm instantiate)", "-", "cold", cold, { bytes: wasm.length });
    const warm = [];
    for (let i = 0; i < N.warm; i++) { const p = await openPage(warmCtx); warm.push(await p.evaluate(() => initSdk())); await p.evaluate(() => sdkDispose()); await p.close(); }
    add("init (worker + wasm instantiate)", "-", "warm", warm);
    const page = await openPage(warmCtx); await page.evaluate(() => initSdk());
    for (const f of fixtures) {
      const b64 = f.buf.toString("base64"), opens = [], closes = []; let pages;
      try {
        for (let i = 0; i < N.op; i++) { const o = await page.evaluate((b) => sdkOpen(b), b64); opens.push(o.ms); pages = o.pages; if (i < N.op - 1) closes.push(await page.evaluate(() => sdkClose())); }
        const per = Array.from({ length: pages }, () => []);
        for (let r = 0; r < N.op; r++) for (let p = 0; p < pages; p++) per[p].push((await page.evaluate(({ p, s }) => sdkRender(p, s), { p, s: SCALE })).ms);
        add("open", f.id, "warm", opens, { pages });
        add("renderPage@1.35", f.id, "warm", per.flat(), { pages, perPageMedianMs: per.map((a) => stat1(a).ms) });
        if (f.id === "native-30p") measurements.push({ op: "rss-after-open+render", input: f.id, run: "warm", rssBytes: treeRssBytes() });
        closes.push(await page.evaluate(() => sdkClose()));
        add("close", f.id, "warm", closes);
      } catch (e) { unavailable.push({ op: "open/renderPage", input: f.id, reason: String(e.message).split("\n")[0].slice(0, 200) }); await page.evaluate(() => sdkClose()).catch(() => {}); }
    }
    await page.evaluate(() => sdkDispose()); await warmCtx.close();
  } finally { srv.close(); for (const d of dirs) await rm(d, { recursive: true, force: true }); }
  for (const op of ["text extraction", "search", "text edit (SET_TEXT_EDITS equivalent)", "redaction apply", "export / save"]) unavailable.push({ op, reason: "unavailable: not implemented yet (SDK smoke API is open/pageGeometry/renderPage/close only)" });
  unavailable.push({ op: "JS heap", reason: "performance.measureUserAgentSpecificMemory not available in headless Chromium or WebKit; process-tree RSS recorded instead" });
  return { engine: `foliark@${gitSha()}`, engineNote: `prebuilt packages/sdk/{dist,wasm}; wasm sha256 ${sha256(wasm).slice(0, 16)} (${wasm.length} B); working tree had uncommitted changes by another agent`, browser: `${browserName} ${browserVersion}`, inputs: fixtures.map(({ id, sha256, pages, bytes, origin }) => ({ id, sha256, pages, bytes, origin })), measurements, unavailable, assets: { wasmBytes: wasm.length } };
}
if (import.meta.url === `file://${process.argv[1]}`) console.log(JSON.stringify(await runSdk({ smoke: process.argv.includes("--smoke") }), null, 1));
