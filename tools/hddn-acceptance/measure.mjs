// Fresh numbers on the pinned HDDN build through its own UI (NEW-76): /redact cold load (requests, bytes, time) and, for one native PDF,
// import (setInputFiles -> first text span) and native export (Export click -> file handed over). UI-level timings, NOT the worker-op
// timings of docs/performance/comparison.md. Chromium, no service worker, localhost.
//   node tools/hddn-acceptance/measure.mjs HDDN_DIST PDF KEYWORD [runs]      (memory-queue, --need-gb 3)
import { createServer } from 'node:http';
import { mkdtempSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { extname, join, resolve } from 'node:path';

const [dist, pdf, keyword, runs = '3'] = [...process.argv.slice(2)];
const root = resolve(dist);
const { chromium } = await import(process.env.PDF_PLAYWRIGHT_MODULE ?? 'playwright');
const types = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.wasm': 'application/wasm', '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2' };
const server = createServer(async (req, res) => {
  try {
    let file = resolve(root, '.' + decodeURIComponent(new URL(req.url, 'http://x').pathname));
    const bytes = await readFile(file).catch(() => readFile(join(file, 'index.html')).then((b) => ((file = 'x.html'), b)));
    res.setHeader('Content-Type', types[extname(file)] ?? 'application/octet-stream');
    res.end(bytes);
  } catch { res.statusCode = 404; res.end(); }
});
await new Promise((done) => server.listen(0, '127.0.0.1', done));
const origin = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch();
const median = (a) => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)];
const out = { browser: browser.version(), cold: [], import: [], exportNative: [], exportBytes: [] };
for (let i = 0; i < Number(runs); i++) {
  const context = await browser.newContext({ serviceWorkers: 'block', acceptDownloads: true, viewport: { width: 1400, height: 1000 } });
  const page = await context.newPage();
  await page.addInitScript(() => { delete window.showSaveFilePicker; });
  let bytes = 0, requests = 0;
  page.on('response', async (r) => { requests += 1; bytes += (await r.body().catch(() => Buffer.alloc(0))).length; });
  const t0 = Date.now();
  await page.goto(`${origin}/redact`, { waitUntil: 'networkidle' });
  await page.locator('input[type=file]').first().waitFor({ state: 'attached' });
  out.cold.push({ ms: Date.now() - t0, requests, bytes });
  const t1 = Date.now();
  await page.locator('input[type=file]').first().setInputFiles(resolve(pdf));
  await page.waitForFunction(() => document.querySelectorAll('[data-span-id]').length > 0, null, { timeout: 180000 });
  out.import.push(Date.now() - t1);
  await page.getByPlaceholder(/Add a word or phrase/).fill(keyword);
  await page.keyboard.press('Enter');
  await page.locator('button[data-highlight-focus]').first().waitFor({ timeout: 60000 });
  const download = page.waitForEvent('download', { timeout: 180000 });
  const t2 = Date.now();
  await page.getByRole('button', { name: 'Export', exact: true }).first().click();
  const confirm = page.getByRole('button', { name: 'Confirm all markings' });
  if (await confirm.isVisible({ timeout: 1500 }).catch(() => false)) await confirm.click();
  const file = await download;
  out.exportNative.push(Date.now() - t2);
  out.exportBytes.push((await readFile(await file.path())).length);
  await context.close();
}
out.summary = {
  coldMs: median(out.cold.map((c) => c.ms)), coldRequests: out.cold[0].requests, coldBytes: median(out.cold.map((c) => c.bytes)),
  importMs: median(out.import), exportMs: median(out.exportNative), exportBytes: out.exportBytes[0],
};
console.log(JSON.stringify(out, null, 1));
await browser.close();
await new Promise((done) => server.close(done));
