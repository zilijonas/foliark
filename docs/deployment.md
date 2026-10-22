# Deploying @foliark/sdk

Static files only: no server code, no COOP/COEP, no SharedArrayBuffer, no GPU, no OffscreenCanvas. Everything below is
exercised against the packed tarball by `tests/package-consumers/run.sh` (vanilla, Vite, esbuild under `/` and `/app/`) and
`tests/browser/run.sh` (CSP and offline specs in chromium, firefox, webkit). Version 0.1.0 is private: nothing is published.

## What ships
`pnpm pack` (from `packages/sdk`, after `scripts/build-wasm.sh` and `pnpm build:sdk`) makes `foliark-sdk-0.1.0.tgz`:

| Path in the package | Role |
|---|---|
| `dist/index.js`, `dist/*.d.ts` | ESM API and declarations (`import { createFoliark } from "@foliark/sdk"`) |
| `dist/worker.js` | module worker entry (`@foliark/sdk/worker`); imports `../wasm/foliark_wasm.js` |
| `dist/inline.js` | in-worker entry (`@foliark/sdk/inline`): `createInlineFoliark`, the same client with the engine in the calling thread |
| `wasm/foliark_wasm_bg.wasm` | the engine (`@foliark/sdk/wasm`), 5.25 MB raw, 2.02 MB gzip, 1.54 MB brotli |
| `wasm/foliark_wasm.js` | wasm-bindgen glue, used by the worker only |
| `LICENSE` (placeholder), `NOTICE`, `LICENSE_FOXIT` | notices that must travel with the wasm (embedded Foxit fonts, ICC profiles) |

No runtime dependencies; `sideEffects` is `["./dist/worker.js"]` (false for everything else; the worker entry must survive tree shaking).
`wasm-opt -O1` runs in `scripts/build-wasm.sh` (binaryen from npm, simd128 kept). Measured on the same build, gzip -9 / brotli 11:
none 5,454,243 B raw, 2,033,973 / 1,546,284; -O1 5,253,214, 2,018,637 / 1,543,921; -O3 5,064,800, 2,091,348 / 1,608,430;
-Oz 4,989,028, 2,087,154 / 1,605,326. -O2 and -Os match -O3. Larger optimisation levels shrink the raw file and grow what is transferred, so -O1.

## Wiring (spec 1)
`createFoliark({ wasmUrl, workerUrl })` or `{ wasmUrl, worker: () => new Worker(...) }`. Nothing is fetched until `createFoliark` runs, so call it
on first use, not at page load. Both URLs are explicit; the SDK never guesses a location. Failure to load either is `ASSET_LOAD_FAILED`.

- **No bundler** (`examples/vanilla`): copy `dist/` and `wasm/` next to the page as `sdk/`, then
  `workerUrl: new URL("./sdk/dist/worker.js", import.meta.url)`, `wasmUrl: new URL("./sdk/wasm/foliark_wasm_bg.wasm", import.meta.url)`.
  Relative URLs make the same files work at `/` and at `/app/`.
- **Vite / Astro (HDDN)**: import the entries as URLs; Vite hashes them and prefixes `base`. Set `worker: { format: "es" }`.
  ```ts
  import { createFoliark } from "@foliark/sdk";
  import wasmUrl from "@foliark/sdk/wasm?url";
  import workerUrl from "@foliark/sdk/worker?worker&url";
  const foliark = await createFoliark({ wasmUrl, workerUrl });
  ```
  Put the same two URLs in the offline precache list (HDDN's `airplane-assets.ts` already imports its worker as `?worker&url`).
- **esbuild**: bundle `@foliark/sdk/worker` as a second entry point (`dist/worker.js` in the output), import the wasm with `loader: { ".wasm": "file" }`
  and `publicPath` set to the base, and pass `workerUrl: new URL("./worker.js", import.meta.url)`. TypeScript needs
  `declare module "@foliark/sdk/wasm" { const url: string; export default url; }` (the build tools above provide their own).
- Other bundlers: any setup that yields two final URLs works; `new URL("...", import.meta.url)` is not required.

## Inside an existing worker (`@foliark/sdk/inline`)
A host that already runs its own dedicated worker with its own generation and recovery (HDDN) should not nest a second one. Import the inline entry in that worker:
```ts
import { createInlineFoliark } from "@foliark/sdk/inline";
import wasmUrl from "@foliark/sdk/wasm?url"; // the page or worker's bundler resolves it, same asset as above
const foliark = await createInlineFoliark({ wasmUrl, limits: { maxPages: 30 } }); // or { wasm: bytesOrModule } to fetch nothing
```
It returns the same `Foliark` and `DocumentHandle` (same DTOs, same `FoliarkError` codes); only the transport differs (a `MessageChannel` inside the thread, so
buffers still transfer). No `workerUrl`, no `worker.js` asset. The host owns hang supervision (the watchdog is off: a stuck wasm call blocks the host worker too)
and, after any `onReset`, must replace its whole worker: a trap or out-of-memory cannot be healed inside the thread that owns the instance, and wasm memory never
shrinks. Cancel with `AbortSignal` as usual; long ops yield between steps, so a queued cancel is seen.

## Hosting
- Serve `.wasm` as `application/wasm` (streaming compile needs it) and `.js` as a JavaScript type. Plain GETs, no cookies, no range requests.
- Any base path works, `/` or `/app/`; nothing is absolute. Compression: gzip or brotli on `.wasm` and `.js` (brotli saves about 25% over gzip).
- Versioned names: hash or version the file names (`foliark_wasm_bg-<hash>.wasm`, `worker-<hash>.js`) and serve them `Cache-Control: public, max-age=31536000, immutable`.
  The worker and the wasm must come from the same package version; never mix a cached worker with a new wasm. Vite and esbuild
  above produce such names; for hand hosting put the version in the directory (`/assets/foliark/0.1.0/`).
- No third-party origins: a run makes GET requests for the site's own files only, never a request that carries the document (asserted by the specs).

## CSP
The minimal policy is enough, with no `default-src` needed:
```
Content-Security-Policy: script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'
```
`'wasm-unsafe-eval'` is required (without it init fails `ASSET_LOAD_FAILED`, tested). The worker script takes the CSP from its own response, so send the
header on `worker.js` too. Inline scripts and import maps are blocked by this policy, which is why the example uses none. `blob:` is only used for the
download link and needs no directive here; if you add `default-src`, allow `blob:` for the object URL you create.

## Prepared offline
The SDK caches nothing itself. A caller that wants airplane mode caches the engine files, never documents:

```js
// sw.js: cache-first for the engine files (list them from your build: html, js, worker, wasm)
const CACHE = "foliark-0.1.0";
const ASSETS = ["/", "/main.js", "/sdk/dist/worker.js", "/sdk/wasm/foliark_wasm_bg.wasm" /* plus every module the page and worker import */];
self.addEventListener("install", (e) => e.waitUntil(caches.open(CACHE).then((c) => c.addAll(ASSETS)).then(() => self.skipWaiting())));
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));
self.addEventListener("fetch", (e) => { if (e.request.method === "GET") e.respondWith(caches.match(e.request).then((r) => r ?? fetch(e.request))); });
```
Register it once online (`navigator.serviceWorker.register("/sw.js")`), wait for `navigator.serviceWorker.ready`, and later loads work with the network
gone. A new SDK version gets a new `CACHE` name and a precache of the new files; delete old caches on `activate`.
`tests/browser/offline.spec.ts` runs exactly this (the module list is every file of the static site) and checks: reload offline, full flow; Cache Storage holds
only html, js and wasm and none contains document words; IndexedDB and Web Storage are empty; the server saw no request while offline; the same words are in
the input file (negative control). Not prepared and offline: the engine does not start and `createFoliark` rejects `ASSET_LOAD_FAILED` (worker or wasm not
fetchable), promptly (about half a second locally); the document is never touched. Note: WebKit under Playwright fails `page.reload()` under `setOffline` with a service worker, so the spec
stops the origin there instead; real Safari is NEW-64.

## cmapsUrl
`cmapsUrl` (spec 10.3, a base URL for an optional `cmaps/<name>.bcmap` pack) is accepted and passed to the worker, but 0.1.0 neither ships a pack nor fetches one. Leave it
unset. When the pack exists it is a third offline asset: host it beside the wasm, version it the same way and add it to the precache list.

## Checks
`tests/package-consumers/run.sh` (6 builds, chromium) and `tests/browser/run.sh` (21 tests) are the reproducible commands; both take the memory queue from outside, never inside.
