// SDK renders of the synthetic scans (NEW-81): node tools/bench/scans-sdk.mjs [--browser chromium|webkit] [--out FILE]
// Per scan fixture and scale: renderPage right after open (first draw), the second and the third render of the same page
// (the third is what a re-render after an edit or a preview refresh costs once the decode is kept). Uses the prebuilt packages/sdk/{dist,wasm}.
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import playwright from "playwright";
import { BENCH, ROOT, SCAN_FIXTURES, gitSha, loadFixtures, serve, sha256, stat1 } from "./lib.mjs";

export async function runSdkScans({ browserName = "chromium", scales = [1.35, 3], rounds = 5 } = {}) {
  const wasm = await readFile(join(ROOT, "packages/sdk/wasm/foliark_wasm_bg.wasm"));
  const fixtures = await loadFixtures(SCAN_FIXTURES);
  const srv = await serve([{ prefix: "/__bench/", dir: BENCH }, { prefix: "/", dir: ROOT }]);
  const dir = await mkdtemp(join(tmpdir(), "sdk-scans-"));
  const measurements = [];
  try {
    const ctx = await playwright[browserName].launchPersistentContext(dir);
    const browser = `${browserName} ${ctx.browser()?.version() ?? ""}`.trim();
    const page = await ctx.newPage();
    page.on("pageerror", (e) => console.error("pageerror:", e.message));
    await page.goto(`${srv.origin}/__bench/sdk-page.html`);
    await page.waitForFunction(() => window.ready);
    await page.evaluate(() => initSdk());
    for (const f of fixtures) {
      const b64 = f.buf.toString("base64");
      const first = Object.fromEntries(scales.map((s) => [s, []])), second = Object.fromEntries(scales.map((s) => [s, []])), repeat = Object.fromEntries(scales.map((s) => [s, []]));
      for (let i = 0; i < rounds; i++) {
        const { pages } = await page.evaluate((b) => sdkOpen(b), b64);
        // the repeat follows its first render at once: the per-document image store is bounded (16 MiB), so a pass over every page
        // and scale first would measure eviction, not the re-render of one page
        for (let p = 0; p < pages; p++) for (const s of scales) {
          first[s].push((await page.evaluate(({ p, s }) => sdkRender(p, s), { p, s })).ms);
          second[s].push((await page.evaluate(({ p, s }) => sdkRender(p, s), { p, s })).ms);
          repeat[s].push((await page.evaluate(({ p, s }) => sdkRender(p, s), { p, s })).ms);
        }
        await page.evaluate(() => sdkClose());
      }
      for (const s of scales) {
        measurements.push({ op: `renderPage@${s} first after open`, input: f.id, ...stat1(first[s]), pages: f.pages });
        measurements.push({ op: `renderPage@${s} second`, input: f.id, ...stat1(second[s]), pages: f.pages });
        measurements.push({ op: `renderPage@${s} repeat`, input: f.id, ...stat1(repeat[s]), pages: f.pages });
      }
    }
    await page.evaluate(() => sdkDispose());
    await ctx.close();
    return { engine: `foliark@${gitSha()}`, browser, wasmSha256: sha256(wasm), wasmBytes: wasm.length, inputs: fixtures.map(({ id, sha256, pages, bytes }) => ({ id, sha256, pages, bytes })), measurements };
  } finally { srv.close(); await rm(dir, { recursive: true, force: true }); }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const arg = (k) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : undefined; };
  const res = JSON.stringify(await runSdkScans({ browserName: arg("--browser") ?? "chromium" }), null, 1);
  if (arg("--out")) await writeFile(arg("--out"), res); else console.log(res);
}
