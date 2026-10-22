// Headless-browser smoke: PDF -> worker -> wasm -> RGBA pixels. Usage: node tests/smoke/run.mjs [chromium|webkit|firefox]
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import playwright from "playwright";
import { busyPdf } from "./busy-pdf.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const types = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm", ".pdf": "application/pdf" };

// 30M "q Q" pairs = 120 MB decoded, under the 128 MiB stream cap: seconds of interpretation in one call.
const virtual = { "/busy.pdf": () => busyPdf(30_000_000) };

const server = createServer(async (req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname));
  try {
    if (path.includes("..")) throw new Error("bad path");
    res.writeHead(200, { "content-type": types[extname(path)] ?? "application/octet-stream" });
    res.end(virtual[path]?.() ?? (await readFile(join(root, path))));
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
  assert.deepEqual([r.complete, r.warnings, r.clip], [true, [], { x: 0, y: 0, width: 200, height: 100 }]);
  assert.deepEqual(
    [r.part.width, r.part.height, r.part.clip, r.part.red, r.part.corner],
    [200, 200, { x: 100, y: 0, width: 100, height: 100 }, [255, 0, 0, 255], [0, 0, 0, 0]],
  );
  assert.equal(r.outOfRange, "INVALID_ARGUMENT");
  assert.equal(r.pdfVersion, "1.7");
  assert.equal(r.helloText.trim(), "Hello Foliark");
  assert.deepEqual([r.fixture.pages, r.fixture.hits > 0, r.fixture.sliced, r.fixture.closedCode], [4, true, true, "DOCUMENT_GONE"]);
  assert.match(r.fixture.text, /KEEP_PUBLIC_ROTATION_CROP/);
  assert.equal(r.fixture.render[2], r.fixture.render[0] * r.fixture.render[1] * 4);
  assert.equal(r.refused, "CONSENT_REQUIRED", "raster export needs consent (and is not built until NEW-54)");
  assert.equal(r.exported.length, r.exported.planLength);
  assert.equal(r.exported.text.trim(), "Hello Foliark", "native export reopens with its text");
  assert.equal(r.exported.consumed, "PLAN_CONSUMED");
  console.log(`smoke ${name}: ok (${r.width}x${r.height}, ${r.nonWhite} non-white px)`);

  // Hard recovery in a real worker: a synchronous call that never returns is terminated by the watchdog.
  const hang = await browser.newPage();
  hang.on("pageerror", (e) => console.error("pageerror:", e.message));
  await hang.goto(`http://127.0.0.1:${server.address().port}/tests/smoke/hang.html`);
  await hang.waitForFunction(() => window.__hang, null, { timeout: 240_000 });
  const h = await hang.evaluate(() => window.__hang);
  assert.equal(h.error, undefined, h.error);
  assert.ok(h.fullMs > 3 * h.openMs, `the op must be long next to open: ${JSON.stringify(h)}`);
  assert.deepEqual([h.stuck, h.queued], ["WORKER_RESET", "WORKER_RESET"], "stuck and queued calls reject WORKER_RESET");
  assert.ok(h.stuckMs >= h.hangMs && h.stuckMs < h.fullMs, `terminated after the timeout, before the op would end: ${JSON.stringify(h)}`);
  assert.deepEqual([h.generation, h.resets, h.gone], [2, [{ generation: 2, reason: "hang" }], "DOCUMENT_GONE"]);
  assert.deepEqual([h.after, h.afterGeneration, h.disposed], ["Hello Foliark", 2, "CLIENT_DISPOSED"]);
  console.log(`smoke ${name}: hang ok (op ${Math.round(h.fullMs)} ms, watchdog ${h.hangMs} ms, reset after ${Math.round(h.stuckMs)} ms)`);
} finally {
  await browser.close();
  server.close();
}
