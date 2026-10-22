// The full flow under the minimal CSP of spec 10.3 and no COOP/COEP, on the packed package's static files.
import { expect, test } from "playwright/test";
import { MINIMAL_CSP, runFlow, serveSite } from "./site.js";

const reports = (hits: { url: string; body: string }[]) => hits.filter((h) => h.url === "/csp-report").map((h) => h.body);

test("full flow under script-src 'self' 'wasm-unsafe-eval'; worker-src 'self', no violations, no isolation headers", async ({ page }) => {
  const site = await serveSite({ csp: `${MINIMAL_CSP}; report-uri /csp-report` });
  const headers: Record<string, string>[] = [];
  const errors: string[] = [];
  page.on("response", (r) => headers.push(r.headers()));
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  try {
    await page.goto(site.origin);
    expect(await page.evaluate(() => crossOriginIsolated)).toBe(false);
    await runFlow(page);
    expect(reports(site.hits), "CSP violation reports").toEqual([]);
    expect(errors).toEqual([]);
    expect(headers.length).toBeGreaterThan(10);
    for (const h of headers) {
      expect(h["content-security-policy"]).toContain("worker-src 'self'");
      expect(h["cross-origin-opener-policy"]).toBeUndefined();
      expect(h["cross-origin-embedder-policy"]).toBeUndefined();
    }
    // Negative control: the policy is enforced and violations do reach the report endpoint.
    await page.evaluate(() => {
      const s = document.createElement("script");
      s.textContent = "window.__inline = 1";
      document.head.append(s);
    });
    expect(await page.evaluate(() => "__inline" in window)).toBe(false);
    await expect.poll(() => reports(site.hits).length).toBeGreaterThan(0);
  } finally {
    site.close();
  }
});

test("without 'wasm-unsafe-eval' the engine refuses to start with a typed error, not a hang", async ({ page }) => {
  const site = await serveSite({ csp: "script-src 'self'; worker-src 'self'" });
  try {
    await page.goto(site.origin);
    await page.setInputFiles("#file", new URL("../fixtures/hddn/metadata-xmp.pdf", import.meta.url).pathname);
    await expect(page.locator("#status")).toHaveText(/^ASSET_LOAD_FAILED: /, { timeout: 30_000 });
  } finally {
    site.close();
  }
});
