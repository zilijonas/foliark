# @foliark/sdk

Unreleased (0.1.0), AGPL-3.0-only with separate commercial licensing; see `LICENSE`, `LICENSING.md` and `COMMERCIAL-LICENSING.md`. A headless Rust/WASM PDF engine for the browser: extract text, search, render,
edit paragraphs, redact and export a sanitized PDF. It runs in a worker on the user's device; the document never leaves it.
The SDK fetches only its own `.wasm` and worker script.

## Install

From a tarball built by this repository (nothing is published):

```sh
npm install ./foliark-sdk-0.1.0.tgz        # or pnpm add / yarn add with the same path
```

ESM only, no runtime dependencies. The package contains `dist/` (API and worker), `wasm/` (the engine, about 5.4 MB raw, 2.1 MB gzip),
`LICENSE`, `LICENSE_FOXIT`, `NOTICE`, `THIRD_PARTY_NOTICES.md` and `SUPPORT_MATRIX.md`.

## Start it

`createFoliark` starts a module worker that owns the wasm instance. Both URLs are explicit; nothing is fetched until you call it.

```ts
import { createFoliark } from "@foliark/sdk";
import wasmUrl from "@foliark/sdk/wasm?url";                 // Vite; other bundlers: any setup that yields the two final URLs
import workerUrl from "@foliark/sdk/worker?worker&url";

const foliark = await createFoliark({ wasmUrl, workerUrl, limits: { maxPages: 200 } });
```

If your app already runs its own dedicated worker with its own recovery (HDDN does), use the in-worker entry instead. Same API and errors,
no second worker, no `worker.js` asset; your worker supervises hangs and must be replaced after `onReset`:

```ts
import { createInlineFoliark } from "@foliark/sdk/inline";
const foliark = await createInlineFoliark({ wasmUrl });     // or { wasm: bytesOrModule } to fetch nothing
```

## Use it

```ts
import { FoliarkError, type Redaction } from "@foliark/sdk";

const doc = await foliark.open(await file.arrayBuffer());   // password?, allowRepaired? in the options; the buffer is transferred
const { pageCount } = await doc.info();

const page = await doc.extractText(0);                      // text, words with boxes, styles; coordinates are display space
const hits = await doc.search("invoice", { caseSensitive: false });
const png = await doc.renderPage(0, { scale: 1.5 });        // RGBA pixels (ImageData-compatible), plus warnings and `complete`

// Edit one paragraph: register a font once, then replace the text of the lines under the rectangles.
await foliark.registerFonts([{ bytes: fontBytes, family: "Liberation Sans", weight: 400, style: "normal" }]);
const info = await doc.prepareParagraph(0, [page.words[0].box]);
let r = await doc.applyChanges({ baseRevision: doc.revision, textEdits: [{ id: "e1", pageIndex: 0, rects: [info.bounds], text: "New text" }] });
if (!r.accepted) console.warn(r.rejected);                  // typed codes per edit, nothing changed

// Redact text found by search, then export a sanitized native PDF.
const redactions: Redaction[] = hits.map((h, i) => ({ kind: "text", id: `r${i}`, pageIndex: h.pageIndex, start: h.start, end: h.end }));
r = await doc.applyChanges({ baseRevision: doc.revision, redactions });
try {
  const plan = await doc.prepareExport({ mode: "native" }); // does the whole export; plan.report lists what was removed or flattened
  const pdf = await doc.export(plan);                       // Uint8Array, single use
} catch (e) {
  if (e instanceof FoliarkError) console.error(e.code, e.details);  // e.g. FORM_APPEARANCE_MISSING, REDACT_PATTERN_UNSUPPORTED
}

// Image-only export burns the redactions into pixels. It drops text, search and vectors, so it needs consent.
const flat = await doc.export(await doc.prepareExport({ mode: "raster", policy: { raster: { acknowledgeLosses: true, scale: 2 } } }));

await doc.close();                                          // idempotent; frees the document and its caches
await foliark.dispose();
```

Every call that can take time accepts `signal?: AbortSignal` (abort rejects `CANCELLED`, state unchanged). Every failure is a `FoliarkError`
with a `code` from the frozen list in `docs/spec/v1.md` section 12; unsupported input is refused with a code, never silently degraded.
`ChangeSet`, `TextEdit`, `Redaction`, `ExportReport`, `Loss` and the other DTOs are exported as types.

## Hosting, CSP, offline

Static files only: serve `wasm/foliark_wasm_bg.wasm` as `application/wasm`, version the file names, and use
`Content-Security-Policy: script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'`. Prepared offline means your service worker caches the
wasm, worker and your own files; the SDK caches nothing and no document is ever stored. Bundler recipes (Vite, esbuild, no bundler), CSP and
the offline pattern are in the repository's `docs/deployment.md`.

## What is supported

`SUPPORT_MATRIX.md` (in the package; `docs/support-matrix.md` in the repository) lists each browser and feature as supported, refused with a
typed code, or not established, with the evidence behind it. Real iPhone, iPad and Android devices are not established yet.

## Limits worth knowing

- Defaults: 64 MiB input, 1000 pages, 50 Mpx per render, 256 MiB output, 4 open documents (`limits` option; spec section 9). Exceeding one is `LIMIT_EXCEEDED`, no output.
- Repaired or truncated files are refused (`REPAIRED_PDF`) unless you pass `allowRepaired`; native export always rewrites the file from scratch and
  drops metadata, attachments, scripts, outlines and similar items, reporting each in `plan.report`.
- Paragraph editing is horizontal, unrotated, left-to-right text only, and needs a registered font when the document's own font cannot be reused.
- `cmapsUrl` is reserved: no CMap pack ships, so fonts that need a predefined CJK CMap render with a `CMAP_MISSING` warning.
- Redaction is verified inside the engine and by independent oracles on 80 test documents; it is not a certificate for arbitrary real-world files.
