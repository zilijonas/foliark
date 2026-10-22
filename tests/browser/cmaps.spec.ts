// NEW-88, packed package in a real browser: the shipped CMap pack is found with no cmapsUrl, a document whose fonts use
// predefined CMaps extracts, renders, redacts and exports, a missing or corrupt pack file is a typed failure at open,
// and once prepared the whole flow runs with the origin gone and every foreign request refused.
import { expect, test, type Page } from "playwright/test";
import { serveSite } from "./site.js";

const CJK = new URL("../fixtures/generated/cjk-predefined-cmaps.pdf", import.meta.url).pathname;
const NEEDED = ["Adobe-GB1-UCS2", "UniGB-UCS2-H", "UniJIS-UCS2-H"];
const WORD = "文测";
const cmapGets = (hits: { url: string }[]) => hits.map((h) => h.url).filter((u) => u.endsWith(".cmap")).sort();

async function ink(page: Page) {
  return page.evaluate(() => {
    const c = document.getElementById("page") as HTMLCanvasElement;
    const d = c.getContext("2d")!.getImageData(0, 0, c.width, c.height).data;
    let n = 0;
    for (let i = 0; i < d.length; i += 4) if (d[i] < 64) n++;
    return n;
  });
}

/** Open the CJK fixture, check text and pixels, redact the word, download, reopen the export and check it again. */
async function cjkFlow(page: Page) {
  await page.setInputFiles("#file", CJK);
  await expect(page.locator("#redact")).toBeEnabled({ timeout: 60_000 });
  const text = (await page.textContent("#text"))!;
  expect(text.split(/\s+/).filter(Boolean)).toEqual(["KEEP_PUBLIC_CJK", "中文测试", "日本語", "你好"]);
  // seven 21.6 x 19.2 pt boxes at least, drawn from the embedded font through the CMap's CIDs
  expect(await ink(page)).toBeGreaterThan(7 * 21 * 19);
  await page.fill("#word", WORD);
  await page.click("#redact");
  await expect(page.locator("#download")).toBeVisible({ timeout: 60_000 });
  const [download] = await Promise.all([page.waitForEvent("download"), page.click("#download")]);
  const chunks: Buffer[] = [];
  for await (const c of await download.createReadStream()) chunks.push(c as Buffer);
  const out = Buffer.concat(chunks);
  expect(out.subarray(0, 5).toString()).toBe("%PDF-");
  await page.setInputFiles("#file", { name: "out.pdf", mimeType: "application/pdf", buffer: out });
  await expect(page.locator("#text")).not.toContainText(WORD[0], { timeout: 60_000 });
  const after = (await page.textContent("#text"))!;
  expect(after).toContain("日本語"); // the rest of the export is still searchable text
  expect(after).toContain("中");
  expect(after).not.toContain(WORD[1]);
}

test("default pack location: only the three needed files are fetched, then the whole flow works", async ({ page }) => {
  const site = await serveSite();
  const foreign: string[] = [];
  page.on("request", (r) => new URL(r.url()).origin !== site.origin && !/^(blob|data):/.test(r.url()) && foreign.push(r.url()));
  try {
    await page.goto(site.origin);
    await cjkFlow(page);
    expect(cmapGets(site.hits)).toEqual(NEEDED.map((n) => `/sdk/cmaps/${n}.cmap`).sort());
    expect(foreign).toEqual([]);
    for (const h of site.hits) expect([h.method, h.bodyBytes, h.url.includes("?")], h.url).toEqual(["GET", 0, false]);
  } finally {
    site.close();
  }
});

for (const [what, o] of [["missing", { hide: /UniJIS-UCS2-H\.cmap$/ }], ["corrupt", { corrupt: /UniJIS-UCS2-H\.cmap$/ }]] as const) {
  test(`a ${what} pack file is ASSET_LOAD_FAILED at open, naming the map`, async ({ page }) => {
    const site = await serveSite(o);
    try {
      await page.goto(site.origin);
      await page.setInputFiles("#file", CJK);
      await expect(page.locator("#status")).toHaveText(/^ASSET_LOAD_FAILED: .*UniJIS-UCS2-H/, { timeout: 30_000 });
      await expect(page.locator("#redact")).toBeDisabled();
    } finally {
      site.close();
    }
  });
}

test("prepared offline: reload with the origin gone and no foreign request, open, redact and export", async ({ browser, browserName }) => {
  const site = await serveSite();
  const context = await browser.newContext({ acceptDownloads: true });
  // Interception (context.route) would take Firefox's service worker navigation away, so foreign requests are observed
  // instead: offline mode refuses them and any attempt still shows up here.
  const blocked: string[] = [];
  context.on("request", (r) => new URL(r.url()).origin !== site.origin && !/^(blob|data):/.test(r.url()) && blocked.push(r.url()));
  const page = await context.newPage();
  try {
    await page.goto(site.origin);
    await page.evaluate(() => navigator.serviceWorker.register("/sw.js"));
    await page.evaluate(() => navigator.serviceWorker.ready.then(() => undefined));
    await page.reload();
    await expect.poll(() => page.evaluate(() => navigator.serviceWorker.controller !== null)).toBe(true);
    const online = site.hits.length;
    // Same offline mechanics as offline.spec.ts (WebKit cannot reload under setOffline with a service worker).
    if (browserName === "webkit") await site.stop();
    else await context.setOffline(true);
    await page.reload();
    await cjkFlow(page);
    expect(site.hits.slice(online).filter((h) => h.url !== "/sw.js").map((h) => h.url), "requests that reached the server").toEqual([]);
    expect(blocked).toEqual([]);
    const cached = await page.evaluate(async () => (await (await caches.open("foliark-offline-1")).keys()).map((r) => new URL(r.url).pathname).filter((p) => p.endsWith(".cmap")));
    expect(cached).toHaveLength(63); // the whole pack was prepared, whatever the document needs
  } finally {
    await context.close();
    site.close();
  }
});
