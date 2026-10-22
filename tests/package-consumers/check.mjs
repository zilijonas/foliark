// check.mjs name:base:dir ... : serve each build statically at its base and drive it in chromium:
// lazy init, open a file, extract, render, redact, export, and every request stays on the origin.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import playwright from "playwright";

const fixture = fileURLToPath(new URL("../fixtures/hddn/metadata-xmp.pdf", import.meta.url));
const SECRET = "SECRET_METADATA_XMP";
const KEEP = "KEEP_PUBLIC_METADATA_XMP";
const types = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm" };

async function serve(base, dir) {
  const server = createServer(async (req, res) => {
    const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname));
    try {
      if (!path.startsWith(base) || path.includes("..")) throw new Error("outside base");
      let rel = path.slice(base.length);
      if (rel === "" || rel.endsWith("/")) rel += "index.html";
      const body = await readFile(join(dir, rel));
      res.writeHead(200, { "content-type": types[extname(rel)] ?? "application/octet-stream" });
      res.end(body);
    } catch {
      res.writeHead(404).end();
    }
  }).listen(0, "127.0.0.1");
  await new Promise((r) => server.once("listening", r));
  return server;
}

const browser = await playwright.chromium.launch();
try {
  for (const spec of process.argv.slice(2)) {
    const [name, base, dir] = spec.split(":");
    const server = await serve(base, dir);
    const origin = `http://127.0.0.1:${server.address().port}`;
    const page = await browser.newPage();
    const urls = [];
    const failures = [];
    const workers = [];
    page.on("worker", (w) => workers.push(w.url()));
    page.on("request", (r) => urls.push(r.url()));
    page.on("response", (r) => r.status() >= 400 && failures.push(`${r.status()} ${r.url()}`));
    page.on("pageerror", (e) => failures.push(`pageerror ${e.message}`));
    page.on("console", (m) => m.type() === "error" && failures.push(`console ${m.text()}`));

    await page.goto(origin + base);
    await page.waitForLoadState("networkidle");
    assert.deepEqual([...urls.filter((u) => u.endsWith(".wasm")), ...workers], [], `${name} ${base}: wasm fetched or worker started before a file was chosen`);

    await page.setInputFiles("#file", fixture);
    await page.waitForFunction(() => !document.getElementById("redact").disabled, null, { timeout: 60_000 }).catch(async (e) => {
      throw new Error(`${name} ${base}: no page shown, status "${await page.textContent("#status")}" failures ${failures}; ${e.message}`);
    });
    assert.equal(workers.length, 1, `${name} ${base}: one worker`);
    const text = await page.textContent("#text");
    assert.ok(text.includes(SECRET) && text.includes(KEEP), `${name} ${base}: extracted text lacks the fixture words`);
    const inked = await page.evaluate(() => {
      const c = document.getElementById("page");
      const d = c.getContext("2d").getImageData(0, 0, c.width, c.height).data;
      let n = 0;
      for (let i = 0; i < d.length; i += 4) if (d[i] < 128) n++;
      return n;
    });
    assert.ok(inked > 50, `${name} ${base}: page 1 render has no ink (${inked})`);

    await page.fill("#word", SECRET);
    await page.click("#redact");
    await page.waitForFunction(() => !document.getElementById("download").hidden, null, { timeout: 60_000 });
    const out = Buffer.from(await page.evaluate(async () => [...new Uint8Array(await (await fetch(document.getElementById("download").href)).arrayBuffer())]));
    assert.equal(out.subarray(0, 5).toString(), "%PDF-");
    const input = await readFile(fixture);
    assert.ok(input.includes(SECRET), "negative control: the input holds the word in the clear");
    assert.ok(!out.includes(SECRET), `${name} ${base}: exported file still holds ${SECRET}`);

    const external = urls.filter((u) => !u.startsWith(origin + base) && !/^(blob|data):/.test(u));
    assert.deepEqual(external, [], `${name} ${base}: requests outside origin+base`);
    assert.deepEqual(failures, [], `${name} ${base}: failed requests or errors`);
    const assets = urls.filter((u) => /\.(wasm|js)$/.test(u)).map((u) => u.slice(origin.length));
    console.log(`ok ${name.padEnd(8)} ${base.padEnd(5)} ${inked} ink px, export ${out.length} B, ${urls.length} requests, 0 external; ${assets.join(" ")}`);
    await page.close();
    server.close();
  }
} finally {
  await browser.close();
}
