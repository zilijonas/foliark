// `@foliark/sdk/inline`: the engine inside the calling thread, no nested worker, same DTOs and typed errors.
import { readFileSync } from "node:fs";
import { afterEach, expect, it } from "vitest";
import { FoliarkError, type Foliark } from "../src/index.js";
import { createInlineFoliark } from "../src/inline.js";

const wasm = readFileSync(new URL("../wasm/foliark_wasm_bg.wasm", import.meta.url));
const fixture = (name: string) => new Uint8Array(readFileSync(new URL(`../../../tests/fixtures/hddn/${name}`, import.meta.url)));
const clients: Foliark[] = [];
afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.dispose()));
});

it("opens, extracts, searches and renders without a worker, and refuses with typed errors", async () => {
  const f = await createInlineFoliark({ wasm, limits: { maxPages: 30 } });
  clients.push(f);
  const bytes = fixture("rotation-crop.pdf");
  const buf = bytes.slice().buffer;
  const doc = await f.open(buf);
  expect(buf.byteLength).toBe(0); // transferred through the channel, as to a real worker
  const [pages, text] = await Promise.all([doc.pages(), doc.extractText(0)]);
  expect(pages).toHaveLength(4);
  expect(text.words.length).toBeGreaterThan(0);
  expect((await doc.search(text.words[0].text)).length).toBeGreaterThan(0);
  const png = await doc.renderPage(0, { scale: 0.5 });
  expect(png.data.length).toBe(png.width * png.height * 4);

  await doc.close();
  await expect(doc.extractText(0)).rejects.toMatchObject({ code: expect.stringMatching(/DOCUMENT_(GONE|CLOSED)/) });
  const err = await f.open(new TextEncoder().encode("not a pdf").buffer).catch((e) => e);
  expect(err).toBeInstanceOf(FoliarkError);
  expect(err.code).toBe("MALFORMED_PDF");
});
