<p align="center">
  <img src="https://raw.githubusercontent.com/zilijonas/foliark/main/logo.png" alt="Foliark" width="320">
</p>

<p align="center"><b>A PDF engine in Rust and WebAssembly for the browser. Extract, search, render, edit, redact and sanitize PDFs on the user's device. The document never leaves it.</b></p>

```sh
npm install @zilijonas/foliark@0.1.6 --save-exact
```

ESM only, no runtime dependencies, typed. Runs in a worker, fetches only its own `.wasm` and worker script, and refuses what it cannot do safely with a typed `FoliarkError` instead of degrading quietly.

## Why I built it

I run HDDN ([hddn.app](https://hddn.app)), a PDF redaction tool that works entirely in the browser. It used PyMuPDF compiled into Pyodide. That worked, but the engine was a 30 MB download, it took about 1.6 seconds just to start, and it kept missing things I needed.

So I wrote a replacement from scratch: a Rust engine compiled to WebAssembly, built on the [hayro](https://github.com/LaurenzV/hayro) PDF crates. It starts in 19 ms, downloads about 2.2 MB gzipped, and the export is rebuilt from an allowlist, so the file is built to carry nothing you removed. HDDN runs on it now.

Foliark is not a fork, port or modified version of PyMuPDF or MuPDF, and contains none of their code. It is not affiliated with or endorsed by Artifex, whose products those are.

## The numbers, and what they are

Same input files for both engines. Chromium 151 on an Apple M4 Mac mini, served from localhost, medians. The left column is HDDN's build of PyMuPDF in Pyodide, the right is the Foliark SDK in a worker.

| | HDDN: PyMuPDF in Pyodide | Foliark |
|---|---|---|
| Engine download, cold | 30.4 MB | 5.4 MB (2.2 MB gzip, as shipped) |
| Start, cold | 1,596 ms | 19 ms |
| Render one text page | 16.4 ms | 4.3 ms |
| Open, extract and render 30 pages | 1,001 ms | 223 ms |
| Apply a paragraph edit | 1,004 ms | 69 ms |
| Native export, 10 redaction regions | 399 ms | 249 ms |
| Wasm memory after all of it | 250 MB | 75 MB |
| Export size, keyword redaction, 30 pages | 28,747 B | 27,500 B |

Read this before quoting any of it. HDDN's render time includes PNG encoding and Foliark hands back raw RGBA, so that row is not the same work. The edit time includes HDDN regenerating previews. And it is not faster everywhere: the first render of a progressive-JPEG colour scan is about 0.9x, slightly slower (82.5 ms against 76 ms). The other scan types I tried were 1.3x to 5.7x faster at scale 1.35. This is synthetic files on one machine, and phones are not measured. Method, caveats and raw data: [docs/performance/comparison.md](https://github.com/zilijonas/foliark/blob/main/docs/performance/comparison.md).

## What it does

- **Read**: text with word boxes and styles, search, hit test and selection, and encrypted input (RC4 and AES, with the password).
- **Render**: pages to RGBA, with warnings when something could not be drawn exactly.
- **Edit**: replace a paragraph, laid out in Rust with real font shaping. Horizontal, unrotated, left-to-right text.
- **Redact for real**: text, images, vectors (cut precisely, not dropped whole) and form fields are removed from the file, not covered with a box.
- **Sanitize on export**: a fresh file, written from scratch. No metadata, attachments, scripts or orphan objects, with a report of what went. Independent oracles that do not import Foliark check the output.
- **Image-only export**: redactions burned into pixels. It drops text, search and vectors, so it asks for explicit consent.
- **Offline and CSP-friendly**: static files only, no network after they are cached, no SharedArrayBuffer, no COOP/COEP.
- **Legacy CJK encodings**: the predefined CMaps ship in the package.

## Quick start

`createFoliark` starts a module worker that owns the wasm instance. Both URLs are explicit and nothing is fetched until you call it.

```ts
import { createFoliark, FoliarkError, type Redaction } from "@zilijonas/foliark";
import wasmUrl from "@zilijonas/foliark/wasm?url";                 // Vite; any bundler that yields the two final URLs works
import workerUrl from "@zilijonas/foliark/worker?worker&url";

const foliark = await createFoliark({ wasmUrl, workerUrl, limits: { maxPages: 200 } });

const doc = await foliark.open(await file.arrayBuffer());   // password?, allowRepaired? in the options; the buffer is transferred
const { pageCount } = await doc.info();

const page = await doc.extractText(0);                      // text, words with boxes, styles; coordinates are display space
const hits = await doc.search("invoice", { caseSensitive: false });
const pixels = await doc.renderPage(0, { scale: 1.5 });     // RGBA pixels (ImageData-compatible), plus warnings and `complete`

// Redact every hit, then export a sanitized native PDF.
const redactions: Redaction[] = hits.map((h, i) => ({ kind: "text", id: `r${i}`, pageIndex: h.pageIndex, start: h.start, end: h.end }));
await doc.applyChanges({ baseRevision: doc.revision, redactions });
try {
  const plan = await doc.prepareExport({ mode: "native" }); // does the whole export; plan.report lists what was removed or flattened
  const pdf = await doc.export(plan);                       // Uint8Array, single use
} catch (e) {
  if (e instanceof FoliarkError) console.error(e.code, e.details);  // e.g. FORM_APPEARANCE_MISSING, REDACT_PATTERN_UNSUPPORTED
}

await doc.close();
await foliark.dispose();
```

Edit a paragraph: register a font once, then replace the text of the lines under a rectangle.

```ts
await foliark.registerFonts([{ bytes: fontBytes, family: "Liberation Sans", weight: 400, style: "normal" }]);
const info = await doc.prepareParagraph(0, [page.words[0].box]);
const r = await doc.applyChanges({ baseRevision: doc.revision, textEdits: [{ id: "e1", pageIndex: 0, rects: [info.bounds], text: "New text" }] });
if (!r.accepted) console.warn(r.rejected);                  // typed code per edit, nothing changed
```

Image-only export burns the redactions into pixels:

```ts
const flat = await doc.export(await doc.prepareExport({ mode: "raster", policy: { raster: { acknowledgeLosses: true, scale: 2 } } }));
```

Already running your own dedicated worker with its own recovery? Use the in-worker entry. Same API and errors, no second worker, and your worker supervises hangs and must be replaced after `onReset`:

```ts
import { createInlineFoliark } from "@zilijonas/foliark/inline";
const foliark = await createInlineFoliark({ wasmUrl });     // or { wasm: bytesOrModule } to fetch nothing
```

Every call that can take time accepts `signal?: AbortSignal` (abort rejects `CANCELLED`, state unchanged). Every failure is a `FoliarkError` with a `code` from the frozen list in the [spec](https://github.com/zilijonas/foliark/blob/main/docs/spec/v1.md) section 12. `ChangeSet`, `TextEdit`, `Redaction`, `ExportReport` and the other DTOs are exported as types.

## Hosting, CSP, offline

Serve `wasm/foliark_wasm_bg.wasm` as `application/wasm`, version the file names, and use `Content-Security-Policy: script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'`. For offline use your service worker caches the wasm, the worker script, the `cmaps/` files and your own assets. The SDK caches nothing and stores no document. Vite, esbuild and no-bundler recipes: [docs/deployment.md](https://github.com/zilijonas/foliark/blob/main/docs/deployment.md).

## Browser support, honestly

Chromium on desktop (macOS arm64) is supported and has budgets. WebKit is supported as an engine, not as Safari. Firefox is functional, with no performance numbers. **Real iPhone, iPad, Android, Safari, Windows and Linux are not tested yet**, and no minimum browser version is promised. Every row and its evidence: [support matrix](https://github.com/zilijonas/foliark/blob/main/docs/support-matrix.md), also in the package as `SUPPORT_MATRIX.md`.

## Limits worth knowing

- Defaults: 64 MiB input, 1000 pages, 50 Mpx per render, 256 MiB output, 4 open documents (`limits` option). Exceeding one is `LIMIT_EXCEEDED`, with no output.
- Repaired or truncated files are refused (`REPAIRED_PDF`) unless you pass `allowRepaired`.
- Native export rewrites the file from scratch and drops metadata, attachments, scripts and outlines, reporting each in `plan.report`.
- Paragraph editing is horizontal, unrotated, left-to-right text only, and needs a registered font when the document's own cannot be reused.
- Redaction is checked inside the engine and by independent oracles on 80 test documents. That is evidence, not a certificate for arbitrary real-world files.

## Links

[Source and issues](https://github.com/zilijonas/foliark) · [Spec](https://github.com/zilijonas/foliark/blob/main/docs/spec/v1.md) · [Deployment](https://github.com/zilijonas/foliark/blob/main/docs/deployment.md) · [Benchmarks](https://github.com/zilijonas/foliark/blob/main/docs/performance/comparison.md) · [Security reports](https://github.com/zilijonas/foliark/blob/main/SECURITY.md) · [Changelog](https://github.com/zilijonas/foliark/blob/main/CHANGELOG.md) · [Provenance](https://github.com/zilijonas/foliark/blob/main/docs/provenance/assets.md)

## License

[AGPL-3.0-only](https://github.com/zilijonas/foliark/blob/main/LICENSE), the same licence family as the AGPL option PyMuPDF is offered under. If the AGPL does not fit your use, separate commercial licensing is available: see `COMMERCIAL-LICENSING.md` in the package. This is not legal advice. Details are in `LICENSING.md`.

Foliark is free software, so the complete corresponding source of version 0.1.6 (Rust engine, vendored hayro fork and patches, build scripts) is tag `v0.1.6` of https://github.com/zilijonas/foliark. Build steps are in `docs/development.md` there (`scripts/build-wasm.sh`, `pnpm build:sdk`, `tests/package-consumers/pack.sh`). The package contains `dist/`, `wasm/` (5.8 MB raw, 2.2 MB gzip), `cmaps/`, `LICENSE`, `LICENSING.md`, `COMMERCIAL-LICENSING.md`, `LICENSE_FOXIT`, `NOTICE`, `THIRD_PARTY_NOTICES.md` and `SUPPORT_MATRIX.md`.

PyMuPDF and MuPDF are products of Artifex Software, Inc. Foliark is independent of them and not affiliated with Artifex.
