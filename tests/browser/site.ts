// Static host for the staged vanilla example (tests/browser/run.sh): records every request the server sees, can add a CSP,
// and serves a tiny service worker (the caller-side offline recipe of docs/deployment.md).
import { createServer } from "node:http";
import { readdir, readFile } from "node:fs/promises";
import { extname, join } from "node:path";
import { expect, type Page } from "playwright/test";

export const SECRET = "SECRET_METADATA_XMP";
export const KEEP = "KEEP_PUBLIC_METADATA_XMP";
export const FIXTURE = new URL("../fixtures/hddn/metadata-xmp.pdf", import.meta.url).pathname;
/** Spec 10.3. */
export const MINIMAL_CSP = "script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'";

const types: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm" };

export type Hit = { method: string; url: string; bodyBytes: number; body: string };

async function walk(dir: string, rel = ""): Promise<string[]> {
  const out: string[] = [];
  for (const e of await readdir(join(dir, rel), { withFileTypes: true })) {
    const p = `${rel}/${e.name}`;
    out.push(...(e.isDirectory() ? await walk(dir, p) : [p]));
  }
  return out;
}

/** The engine files a caller prepares: html, js and wasm; declarations and everything else stay out. */
export const isAsset = (p: string) => /\.(html|js|wasm)$/.test(p);

const SW = (assets: string[]) => `
const CACHE = "foliark-offline-1";
const ASSETS = ${JSON.stringify(["/", ...assets])};
self.addEventListener("install", (e) => e.waitUntil(caches.open(CACHE).then((c) => c.addAll(ASSETS)).then(() => self.skipWaiting())));
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));
self.addEventListener("fetch", (e) => {
  if (e.request.method === "GET") e.respondWith(caches.match(e.request).then((hit) => hit ?? fetch(e.request)));
});
`;

export async function serveSite(o: { csp?: string; hide?: RegExp } = {}) {
  const dir = process.env.FOLIARK_SITE;
  if (!dir) throw new Error("FOLIARK_SITE is not set: run tests/browser/run.sh");
  const files = await walk(dir);
  const hits: Hit[] = [];
  const server = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const c of req) chunks.push(c as Buffer);
    const url = req.url!;
    const body = Buffer.concat(chunks);
    hits.push({ method: req.method!, url, bodyBytes: body.length, body: body.subarray(0, 4096).toString("latin1") });
    const path = new URL(url, "http://x").pathname;
    if (path === "/csp-report") return void res.writeHead(204).end();
    const rel = path === "/" ? "/index.html" : path;
    const headers: Record<string, string> = { "content-type": types[extname(rel)] ?? "application/octet-stream" };
    if (o.csp) headers["content-security-policy"] = o.csp;
    try {
      if (o.hide?.test(path)) throw new Error("hidden");
      const body = path === "/sw.js" ? SW(files.filter(isAsset)) : await readFile(join(dir, rel));
      res.writeHead(200, headers).end(body);
    } catch {
      res.writeHead(404, headers).end();
    }
  }).listen(0, "127.0.0.1");
  await new Promise((r) => server.once("listening", r));
  return { origin: `http://127.0.0.1:${(server.address() as { port: number }).port}`, hits, files, close: () => server.close(),
    /** The origin disappears: connections drop and new ones are refused. */
    stop: () => new Promise<void>((r) => { server.close(() => r()); server.closeAllConnections(); }) };
}

/** The demo flow of examples/vanilla: open, extract, render, redact, download. Returns the downloaded bytes. */
export async function runFlow(page: Page): Promise<Buffer> {
  await page.setInputFiles("#file", FIXTURE);
  await expect(page.locator("#redact")).toBeEnabled({ timeout: 60_000 });
  const text = (await page.textContent("#text"))!;
  expect(text).toContain(SECRET);
  expect(text).toContain(KEEP);
  const ink = await page.evaluate(() => {
    const c = document.getElementById("page") as HTMLCanvasElement;
    const d = c.getContext("2d")!.getImageData(0, 0, c.width, c.height).data;
    let n = 0;
    for (let i = 0; i < d.length; i += 4) if (d[i] < 128) n++;
    return n;
  });
  expect(ink).toBeGreaterThan(50);
  await page.fill("#word", SECRET);
  await page.click("#redact");
  await expect(page.locator("#download")).toBeVisible({ timeout: 60_000 });
  const [download] = await Promise.all([page.waitForEvent("download"), page.click("#download")]);
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const c of stream) chunks.push(c as Buffer);
  const out = Buffer.concat(chunks);
  expect(out.subarray(0, 5).toString()).toBe("%PDF-");
  expect(out.includes(SECRET), "redacted export still holds the word").toBe(false);
  return out;
}
