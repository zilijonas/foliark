// Rollback proof (NEW-76): one Chromium profile loads the CURRENT HDDN build (service-worker cache v16, Foliark), the same origin then serves
// the PRE-Foliark build (2fe0f9a, cache v15, Pyodide) and the profile must recover and redact a PDF with it. Dev-only; never touches a checkout.
//   node tools/hddn-acceptance/rollback.mjs CURRENT_DIST ROLLBACK_DIST FIXTURE.pdf   (run through memory-queue, --need-gb 3)
import { createServer } from 'node:http';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { extname, join, resolve } from 'node:path';
import assert from 'node:assert/strict';

const [currentDist, rollbackDist, fixture] = process.argv.slice(2).map((p) => resolve(p));
const { chromium } = await import(process.env.PDF_PLAYWRIGHT_MODULE ?? 'playwright');
const PORT = 4391;
const origin = `http://127.0.0.1:${PORT}`;
const types = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.wasm': 'application/wasm', '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2', '.zip': 'application/zip' };
let dist = currentDist;
let down = false; // origin unreachable: connections are dropped
const served = [];
const server = createServer(async (req, res) => {
  const path = decodeURIComponent(new URL(req.url, origin).pathname);
  served.push(path);
  if (down) return req.destroy();
  try {
    let file = resolve(dist, '.' + path);
    if (!file.startsWith(dist)) throw new Error('outside');
    let bytes = await readFile(file).catch(() => readFile(join(file, 'index.html')).then((b) => ((file = 'x.html'), b)));
    res.setHeader('Content-Type', types[extname(file)] ?? 'application/octet-stream');
    res.setHeader('Cache-Control', 'no-cache');
    res.end(bytes);
  } catch { res.statusCode = 404; res.end(); }
});
await new Promise((done) => server.listen(PORT, '127.0.0.1', done));

const profile = mkdtempSync(join(tmpdir(), 'rollback-profile-'));
const context = await chromium.launchPersistentContext(profile, { serviceWorkers: 'allow', viewport: { width: 1400, height: 1000 }, acceptDownloads: true });
const page = context.pages()[0] ?? (await context.newPage());
await page.addInitScript(() => { delete window.showSaveFilePicker; });
const cacheState = () => page.evaluate(async () => {
  const out = {};
  for (const name of await caches.keys()) out[name] = (await (await caches.open(name)).keys()).map((r) => new URL(r.url).pathname);
  return out;
});
const prepareOffline = async () => {
  await page.getByRole('button', { name: 'Load offline tools' }).click();
  await page.getByRole('button', { name: 'Download tools' }).click();
  await page.getByText('Offline mode is ready').first().waitFor({ timeout: 240000 });
};
const loadAndExport = async (label) => {
  await page.goto(`${origin}/redact`);
  await page.locator('input[type=file]').first().waitFor({ state: 'attached' });
  await page.locator('input[type=file]').first().setInputFiles(fixture);
  await page.locator('button[data-highlight-focus]').first().waitFor({ timeout: 180000 });
  const download = page.waitForEvent('download', { timeout: 180000 });
  await page.getByRole('button', { name: 'Export', exact: true }).first().click();
  await page.getByRole('button', { name: 'Confirm all markings' }).click();
  const path = join(profile, `${label}.pdf`);
  await (await download).saveAs(path);
  const PY = 'import sys,fitz;d=fitz.open(sys.argv[1]);t=" ".join(p.get_text() for p in d);print(int("rasa.kazlauskaite@example.com" in t.replace("\\n","")), len(d), len(t.split()))';
  const [email, pages, words] = execFileSync('python3', ['-c', PY, path], { encoding: 'utf8' }).trim().split(' ').map(Number);
  assert.equal(email, 0, `${label}: the confirmed email is gone from the saved file`);
  return { pages, words };
};
const result = {};
try {
  // Phase 1: current build, a user who prepared offline tools (cache v16).
  await page.goto(`${origin}/redact`);
  await page.waitForFunction(async () => (await navigator.serviceWorker.getRegistration())?.active, null, { timeout: 60000 });
  await prepareOffline();
  const before = await cacheState();
  result.currentCaches = Object.fromEntries(Object.entries(before).map(([k, v]) => [k, v.length]));
  assert.deepEqual(Object.keys(before), ['document-redactor-airplane-v16']);
  assert.ok(!Object.values(before).flat().some((u) => /pyodide|pymupdf/.test(u)), 'v16 holds no Pyodide');
  result.current = await loadAndExport('current');

  // Phase 2: roll back. Same origin, same profile, the old build's service worker and bundles.
  dist = rollbackDist;
  served.length = 0;
  await page.goto(`${origin}/redact`);
  await page.evaluate(() => navigator.serviceWorker.getRegistration().then((r) => r?.update()));
  const deadline = Date.now() + 120000;
  while (Object.keys(await cacheState()).join() !== 'document-redactor-airplane-v15') {
    assert.ok(Date.now() < deadline, 'the rolled-back service worker (v15) replaced the v16 cache');
    await page.waitForTimeout(500);
  }
  await page.goto(`${origin}/redact`); // now controlled by the v15 worker
  const rolled = await cacheState();
  result.rolledBackCaches = Object.fromEntries(Object.entries(rolled).map(([k, v]) => [k, v.length]));
  result.v15Pyodide = Object.values(rolled).flat().filter((u) => /pyodide\/pyodide\.mjs|\.whl$/.test(u)).length;
  result.carriedOverTesseract = Object.values(rolled).flat().filter((u) => u.startsWith('/tesseract/')).length;
  result.rolledBack = await loadAndExport('rollback');
  result.pyodideFetchedAfterRollback = served.filter((u) => /pyodide\/pyodide\.asm\.wasm|\.whl$/.test(u)).length;
  result.foliarkWasmFetchedAfterRollback = served.filter((u) => /foliark_wasm/.test(u)).length;
  const afterUse = await cacheState();
  result.v15PyodideAfterUse = Object.values(afterUse).flat().filter((u) => /pyodide\/pyodide\.asm\.wasm/.test(u)).length;

  // Phase 3 (exploratory, not part of the pass criteria): offline tools on the rolled-back build. The old dialog did not reach a ready or download state
  // within 60 s in the one run made, so the outcome is recorded, never asserted.
  try {
  await page.goto(`${origin}/redact`);
  await page.getByRole('button', { name: 'Load offline tools' }).click();
  const ready = page.getByText('Offline mode is ready').first();
  const download = page.getByRole('button', { name: 'Download tools' });
  await ready.or(download).first().waitFor({ timeout: 60000 });
  result.offlineReadyBeforeRedownload = await ready.isVisible();
  if (!result.offlineReadyBeforeRedownload) {
    await download.click();
    await ready.waitFor({ timeout: 240000 });
  }
  await page.getByRole('button', { name: 'Close' }).first().click({ timeout: 2000 }).catch(() => {});
  down = true;
  await page.reload();
  await page.locator('input[type=file]').first().waitFor({ state: 'attached' });
  await page.locator('input[type=file]').first().setInputFiles(fixture);
  await page.locator('button[data-highlight-focus]').first().waitFor({ timeout: 180000 });
  result.offlineAfterRollback = 'file opened with the origin down';
  } catch (error) {
    result.offlineAfterRollback = `not established: ${String(error.message).split('\n')[0]}`;
  }
} finally {
  console.log(JSON.stringify(result, null, 1));
  await context.close();
  await new Promise((done) => server.close(done));
}
