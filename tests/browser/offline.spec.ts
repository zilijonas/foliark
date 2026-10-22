// Prepared offline: the caller caches the engine files (a ~6 line service worker, served by site.ts), the network goes
// away, and the flow still works. Nothing of the document is stored or sent. Cold offline fails with a typed error.
import { expect, test, type Page } from "playwright/test";
import { FIXTURE, isAsset, KEEP, runFlow, SECRET, serveSite } from "./site.js";
import { readFile } from "node:fs/promises";

/** Everything a page origin can persist: Cache Storage entries (with a scan for the document's words), IndexedDB, Web Storage. */
async function stored(page: Page) {
  return page.evaluate(async (words) => {
    const entries: { url: string; type: string; length: number; hasDocText: boolean }[] = [];
    for (const name of await caches.keys()) {
      const c = await caches.open(name);
      for (const req of await c.keys()) {
        const res = (await c.match(req))!;
        const text = new TextDecoder("latin1").decode(await res.arrayBuffer());
        entries.push({ url: new URL(req.url).pathname, type: res.headers.get("content-type") ?? "", length: text.length, hasDocText: words.some((w) => text.includes(w)) });
      }
    }
    return { entries, idb: (await indexedDB.databases()).map((d) => d.name), local: localStorage.length, session: sessionStorage.length };
  }, [SECRET, KEEP]);
}

test("prepared offline: reload without network, full flow, nothing of the document stored or sent", async ({ browser, browserName }) => {
  const site = await serveSite();
  const context = await browser.newContext({ acceptDownloads: true });
  const page = await context.newPage();
  try {
    await page.goto(site.origin);
    await page.evaluate(() => navigator.serviceWorker.register("/sw.js"));
    await page.evaluate(() => navigator.serviceWorker.ready.then(() => undefined));
    await page.reload(); // the controlled page; install already cached every file
    await expect.poll(() => page.evaluate(() => navigator.serviceWorker.controller !== null)).toBe(true);
    const online = site.hits.length;

    // WebKit fails page.reload() under setOffline with a service worker ("internal error", Playwright side), so there the
    // origin itself goes away instead. Either way the reload can only be answered by the cache.
    if (browserName === "webkit") await site.stop();
    else await context.setOffline(true);
    await page.reload();
    await runFlow(page);
    expect(site.hits.length, "no request reached the server while offline").toBe(online);

    const s = await stored(page);
    expect(s.idb).toEqual([]);
    expect([s.local, s.session]).toEqual([0, 0]);
    expect(s.entries.map((c) => c.url).sort()).toEqual(["/", ...site.files.filter(isAsset)].sort());
    for (const c of s.entries) {
      expect(c.type, c.url).toMatch(/^(text\/html|text\/javascript|application\/wasm)$/);
      expect(c.hasDocText, `${c.url} holds document text`).toBe(false);
    }
    expect(s.entries.some((c) => c.url.endsWith(".wasm") && c.length > 1e6)).toBe(true);
    // Negative control: the scan's words are really in the document.
    const fixture = (await readFile(FIXTURE)).toString("latin1");
    expect([fixture.includes(SECRET), fixture.includes(KEEP)]).toEqual([true, true]);
  } finally {
    await context.close();
    site.close();
  }
});

test("online run: only GETs of the site's own files, no query, no body, nothing carrying the document", async ({ page }) => {
  const site = await serveSite();
  const external: string[] = [];
  page.on("request", (r) => new URL(r.url()).origin !== site.origin && !/^(blob|data):/.test(r.url()) && external.push(r.url()));
  try {
    await page.goto(site.origin);
    await runFlow(page);
    expect(external).toEqual([]);
    expect(site.hits.length).toBeGreaterThan(10);
    for (const h of site.hits) {
      expect([h.method, h.bodyBytes, h.url.includes("?")], h.url).toEqual(["GET", 0, false]);
      expect(h.url).not.toMatch(new RegExp(`${SECRET}|${KEEP}|metadata-xmp`));
    }
    expect((await stored(page)).entries).toEqual([]);
  } finally {
    site.close();
  }
});

test("cold offline, nothing prepared: a typed error, not a hang", async ({ browser }) => {
  const site = await serveSite();
  const context = await browser.newContext();
  const page = await context.newPage();
  try {
    await page.goto(site.origin);
    await page.waitForLoadState("networkidle");
    await context.setOffline(true);
    await page.setInputFiles("#file", FIXTURE);
    await expect(page.locator("#status")).toHaveText(/^[A-Z_]+: /, { timeout: 30_000 });
    const status = (await page.textContent("#status"))!;
    expect(status).toMatch(/^ASSET_LOAD_FAILED: /);
    await expect(page.locator("#redact")).toBeDisabled();
  } finally {
    await context.close();
    site.close();
  }
});

for (const [what, hide] of [["wasm file", /\.wasm$/], ["worker script", /\/worker\.js$/]] as const) {
  test(`a missing ${what} is ASSET_LOAD_FAILED at init`, async ({ page }) => {
    const site = await serveSite({ hide });
    try {
      await page.goto(site.origin);
      await page.setInputFiles("#file", FIXTURE);
      await expect(page.locator("#status")).toHaveText(/^ASSET_LOAD_FAILED: /, { timeout: 30_000 });
      await expect(page.locator("#redact")).toBeDisabled();
    } finally {
      site.close();
    }
  });
}
