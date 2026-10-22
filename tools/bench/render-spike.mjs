// Render-path spike (NEW-63): ms/page at scale 1.35 for one build variant, plus a phase split measured on the
// main thread against the raw wasm exports. Usage: node tools/bench/render-spike.mjs [--browser webkit] [--n 5]
// Uses the prebuilt packages/sdk/{dist,wasm}. JSON on stdout.
import playwright from "playwright";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { FIXTURES, ROOT, BENCH, loadFixtures, median, serve } from "./lib.mjs";

const arg = (k, d) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : d; };
const browserName = arg("--browser", "chromium"), N = +arg("--n", 5), SCALE = 1.35;
const fixtures = await loadFixtures(FIXTURES.filter((f) => f[0] === "native-small" || f[0] === "native-30p"));
const srv = await serve([{ prefix: "/__bench/", dir: BENCH }, { prefix: "/", dir: ROOT }]);
const browser = await playwright[browserName].launch();
const out = { browser: `${browserName} ${browser.version()}`, wasmBytes: (await readFile(join(ROOT, "packages/sdk/wasm/foliark_wasm_bg.wasm"))).length, fixtures: {} };
try {
  const page = await browser.newPage();
  await page.goto(`${srv.origin}/__bench/sdk-page.html`);
  await page.waitForFunction(() => window.ready);
  await page.evaluate(() => initSdk());
  for (const f of fixtures) {
    const b64 = f.buf.toString("base64");
    const { pages } = await page.evaluate((b) => sdkOpen(b), b64);
    const all = [];
    for (let r = 0; r < N + 1; r++) for (let p = 0; p < pages; p++) { const ms = (await page.evaluate(({ p, s }) => sdkRender(p, s), { p, s: SCALE })).ms; if (r) all.push(ms); }
    await page.evaluate(() => sdkClose());
    const split = await page.evaluate(async ({ b64, N, SCALE }) => {
      const w = await import("/packages/sdk/wasm/foliark_wasm.js");
      await w.default({ module_or_path: "/packages/sdk/wasm/foliark_wasm_bg.wasm" });
      const doc = w.open(Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)));
      const n = w.pageCount(doc), t = { render: [], copy: [] };
      for (let r = 0; r < N + 1; r++) for (let p = 0; p < n; p++) {
        const t0 = performance.now(); const x = w.renderPage(doc, p, SCALE); const t1 = performance.now(); x.takeData(); const t2 = performance.now(); x.free();
        if (r) { t.render.push(t1 - t0); t.copy.push(t2 - t1); }
      }
      t.dump = n > 1 ? [0, 7, 19].map((p) => { const x = w.renderPage(doc, p, SCALE), d = x.takeData(); x.free(); return Array.from(d.subarray(0, 4e6)); }) : [];
      w.close(doc); return t;
    }, { b64, N, SCALE });
    if (arg("--dump") && split.dump.length) await writeFile(arg("--dump"), JSON.stringify(split.dump));
    out.fixtures[f.id] = { pages, endToEndMs: +median(all).toFixed(2), wasmRenderMs: +median(split.render).toFixed(2), dataCopyMs: +median(split.copy).toFixed(2) };
  }
} finally { await browser.close(); srv.close(); }
console.log(JSON.stringify(out));
