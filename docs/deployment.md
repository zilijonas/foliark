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
| `wasm/foliark_wasm_bg.wasm` | the engine (`@foliark/sdk/wasm`), 5.57 MB raw, 2.13 MB gzip (budget in `tools/bench/budgets.json`) |
| `wasm/foliark_wasm.js` | wasm-bindgen glue, used by the worker only |
| `cmaps/<name>.cmap`, `cmaps/LICENSE.txt`, `cmaps/MANIFEST.json` | the predefined CMap pack (`@foliark/sdk/cmaps/*`): 63 Adobe CMap resources, verbatim, BSD-3-Clause, 3190880 B in all; see `## cmapsUrl
Predefined CMaps (legacy CJK encodings such as `UniGB-UCS2-H`, `90ms-RKSJ-H`; spec 10.3) ship in the package as `cmaps/<name>.cmap`: 63 Adobe CMap resources
(63 files, 3,190,880 B raw, about 1.1 MB inside the tarball), copied byte for byte from adobe-type-tools/cmap-resources and mapping-resources-pdf (BSD-3-Clause, notice in
`cmaps/LICENSE.txt` and `THIRD_PARTY_NOTICES.md`; pinned commits and sha256 per file in `cmaps/MANIFEST.json`, rebuilt by `tools/cmaps/build-pack.mjs`). The wasm does
not embed them (about +17 KB raw for the registry and loader only). Common sizes: `UniGB-UCS2-H` 274 KB, `UniJIS-UCS2-H` 169 KB, `UniCNS-UCS2-H` 326 KB, `UniKS-UCS2-H` 166 KB, `UniGB-UTF16-H` 203 KB, `UniJIS-UTF16-H` 188 KB, `90ms-RKSJ-H` 6 KB, `GBK-EUC-H` 83 KB, `Adobe-GB1-UCS2` 221 KB, `Adobe-Japan1-UCS2` 294 KB, `UniGB-UCS2-V` 3 KB; most `-V` and legacy maps are 3 to 30 KB.
They are plain PostScript CMaps rather than hayro's `.bcmap` blob because that format needs the whole embedded 0.25 MB table bundle to decode.

- **Default**: with `cmapsUrl` omitted the worker resolves `../cmaps/` from its own URL, i.e. the package's `cmaps/` next to `dist/` (the no-bundler layout above works unchanged).
  A bundler does not see that URL: copy the folder and set `cmapsUrl` (see the bundler list above). `cmapsUrl` is a folder, absolute or relative to the page.
- **Node**: the default is a `file:` URL there and Node's `fetch` cannot read it, so `@foliark/sdk/inline` takes `fetchCMap: (url) => Promise<Uint8Array>` (for example `async (u) => new Uint8Array(readFileSync(new URL(u)))`); without it `open` rejects `ASSET_LOAD_FAILED`. The worker entry is browser-only and needs no option.
- **When**: `open` reads `DocumentInfo.requiredAssets` (`cmap:<name>`, plus `cmap:Adobe-<Collection>-UCS2` for a CID font the file does not embed), fetches those files
  (and any CMap they `usecmap`, base first) with `credentials: "omit"`, same origin by default, GET only, and registers them before the first page is read. Only file
  names leave the device; a name taken from a PDF becomes a URL only if it is one of the predefined names. Each file is fetched once per worker.
- **Failure**: a file that cannot be fetched, or is truncated, is another map's bytes, or is not a CMap, rejects `open` with `ASSET_LOAD_FAILED`, `details.feature = "cmap:<name>"`
  and a message naming the file and the URL. The document is closed again. No silent fallback: text on those fonts is never guessed.
- **Offline**: the files are versioned static assets; list them in the precache (recipe above) and `open` needs no network. `tests/browser/cmaps.spec.ts` runs the CJK
  fixture reloaded with the origin gone and records zero foreign requests, in chromium, firefox and webkit (webkit stops the origin, see the note above).
- **Still unsupported**: redaction and edit of text in a Type0 font whose encoding mixes code lengths (`90ms-RKSJ-H`, EUC, `Uni*-UTF16-*`) is refused `FONT_UNSUPPORTED`
  (only two-byte `Uni*-UCS2-*` and Identity are rewritten); extraction and render of those fonts work. A font the file does not embed is drawn with a Latin stand-in
  (`FONT_SUBSTITUTED`; CJK glyphs show as missing) while its text still extracts.
