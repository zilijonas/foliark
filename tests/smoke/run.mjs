// Headless-browser smoke: PDF -> worker -> wasm -> RGBA pixels. Usage: node tests/smoke/run.mjs [chromium|webkit|firefox]
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import playwright from "playwright";

const root = fileURLToPath(new URL("../../", import.meta.url));
const types = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm", ".pdf": "application/pdf" };

const server = createServer(async (req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname));
  try {
    if (path.includes("..")) throw new Error("bad path");
    res.writeHead(200, { "content-type": types[extname(path)] ?? "application/octet-stream" });
    res.end(await readFile(join(root, path)));
  } catch {
    res.writeHead(404).end();
  }
}).listen(0, "127.0.0.1");
await new Promise((r) => server.once("listening", r));

const name = process.argv[2] ?? "chromium";
const browser = await playwright[name].launch();
try {
  const page = await browser.newPage();
  page.on("pageerror", (e) => console.error("pageerror:", e.message));
  await page.goto(`http://127.0.0.1:${server.address().port}/tests/smoke/index.html`);
  await page.waitForFunction(() => window.__smoke, null, { timeout: 30_000 });
  const r = await page.evaluate(() => window.__smoke);
  assert.equal(r.error, undefined, r.error);
  assert.equal(r.pageCount, 1);
  assert.deepEqual(r.geometry, {
    index: 0, width: 200, height: 100, rotation: 0, userUnit: 1, mediaBox: [0, 0, 200, 100], cropBox: [0, 0, 200, 100],
    userToDisplay: [1, 0, 0, -1, 0, 100], displayToUser: [1, 0, 0, -1, 0, 100],
  });
  assert.deepEqual([r.width, r.height, r.length], [200, 100, 200 * 100 * 4]);
  assert.deepEqual(r.red, [255, 0, 0, 255], "red rect pixel");
  assert.deepEqual(r.corner, [255, 255, 255, 255], "white background pixel");
  assert.ok(r.nonWhite > 1000, `expected rect + text pixels, got ${r.nonWhite}`);
  assert.match(r.outOfRange, /InvalidArgument/);
  console.log(`smoke ${name}: ok (${r.width}x${r.height}, ${r.nonWhite} non-white px)`);
} finally {
  await browser.close();
  server.close();
}
