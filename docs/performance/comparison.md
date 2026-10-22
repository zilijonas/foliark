# SDK vs HDDN full-op comparison (NEW-62) and what was optimized (NEW-63)

Data: `comparison.json` (schema 1, `kind: foliark-vs-hddn-comparison`): machine, browser versions, git sha and wasm sha256/size, HDDN worker sha256, input sha256s, per-op median/p95/n, bytes, output sizes, PyMuPDF re-read of every exported PDF (`verifiedOutputs`), `quality` notes, `unavailable`.
Same byte-identical inputs for both engines. Apple M4 24 GB, headless, localhost, identity encoding, other agents' jobs were running (loadAvg1mAtStart in the JSON), so treat differences under ~10% as noise.
Reproduce (HDDN worktree built at 2fe0f9a, see README.md; `HDDN_DIST` overrides its dist path):
`~/ai/bin/memory-queue run --owner <id> --label 'foliark compare' --need-gb 2 --restartable --wait 1800 -- node tools/bench/compare.mjs` then `node tools/bench/table.mjs docs/performance/comparison.json`.

## Read this before quoting a speedup
- **Native export is like-for-like on output, not on guarantees.** PyMuPDF re-read: both outputs lose exactly the same 129 words in the same 10 regions (input control: 129), keep all other words (16,761), no Info metadata, 0 embedded files. The SDK writer additionally has the strict-native proof (`strict_native` suite) and fails closed; HDDN has no equivalent and refuses native export for forms, invisible text and optional content. The SDK file is 322,769 B vs 213,136 B.
- **Raster export**: HDDN = PyMuPDF `render_page_png` + pdf-lib; SDK = hayro raster with redactions burned in and losses disclosed. Different rasterizers and codecs (13.4 MB vs 9.1 MB). Both outputs have 30 images and no text layer, 0 words in the redacted regions.
- **Render**: HDDN time includes PNG encoding; the SDK returns raw RGBA. Not the same work.
- **Extraction** has no isolated HDDN op: HDDN `LOAD_PDF` = parse + span extraction + a preview per page, so it is compared only with the SDK composite "open + extract + render@1.35 every page".
- **Search**: HDDN has no plain search; `DETECT` also runs PII rules, name datasets and signatures. Shown for scale, not comparable.
- **Image-heavy HDDN numbers are weak**: 5 of the HDDN `GET_PAGE_PREVIEW` calls on `image-heavy-6p` returned ERROR (unavailable list) and its `LOAD_PDF` takes 11.6 s, so the 6 ms/page median is of the successful calls only. The SDK number (about 100 ms/page) is the honest one to act on; whether HDDN's 6 ms is a cached or reduced-size preview was not established.
- Memory: wasm linear memory (summed `WebAssembly.Memory` byteLength via `tools/bench/memprobe.js` in the worker) is exact; browser RSS is the whole process tree, noisy, and WebKit's value (about 230 to 250 MB for both engines) looks like it misses helper processes, so do not compare RSS across browsers. JS heap is unavailable in headless Chromium/WebKit.
- Paragraph edit: the SDK needs a registered font (Liberation Sans, 410,712 B, 1.3 ms); HDDN fetches the same fallback lazily. Both edit the same line with the same text.

## Results
### chromium 151.0.7922.34

| op (ms, median) | HDDN | Foliark SDK | note |
|---|---|---|---|
| init cold (30.41 MB vs 5.41 MB downloaded) | 1,596 (p95 1,688, n=3) | 18.8 (p95 20.5, n=3) |  |
| init warm (assets cached) | 1,416 (p95 1,463, n=5) | 12.4 (p95 12.6, n=5) |  |
| native-30p: open + extract + render@1.35 every page | 1,001 (p95 1,067, n=5) | 223 (p95 226, n=3) | HDDN LOAD_PDF = parse + extract + page previews |
| native-30p: render@1.35 per page | 16.4 (p95 16.8, n=150) | 4.3 (p95 4.5, n=150) | HDDN encodes PNG, SDK returns RGBA |
| native-30p: first render after open (SDK) | n/a | 4.9 (p95 5.5, n=5) | includes one-time document scan |
| image-heavy-6p: open + extract + render@1.35 every page | 11,565 (p95 11,767, n=5) | 662 (p95 662, n=3) | HDDN LOAD_PDF = parse + extract + page previews |
| image-heavy-6p: render@1.35 per page | 6 (p95 6.3, n=20) | 109 (p95 110, n=30) | HDDN encodes PNG, SDK returns RGBA |
| image-heavy-6p: first render after open (SDK) | n/a | 111 (p95 136, n=5) | includes one-time document scan |
| native-30p: extract all pages | n/a | 28.1 (p95 87.2, n=5) | SDK only (no HDDN op without previews) |
| native-30p: search 'alpha' (SDK) / DETECT (HDDN) | 43.4 (p95 53.6, n=3) | 9.9 (p95 11.1, n=5) | NOT comparable: DETECT also runs PII, names, signatures |
| native-30p: paragraph prepare | 68.2 (p95 69.4, n=3) | 5.5 (p95 6.6, n=3) |  |
| native-30p: paragraph edit apply | 1,004 (p95 1,008, n=3) | 69.1 (p95 72.2, n=3) | HDDN also regenerates previews |
| native-30p: native export, 10 regions | 399 (p95 432, n=3) | 249 (p95 254, n=3) | out 213,136 B vs 322,769 B; same words removed |
| native-30p: raster export @2, 10 regions | 4,132 (p95 4,194, n=3) | 1,029 (p95 1,031, n=3) | out 9,075,784 B vs 13,360,325 B; different rasterizers |
| memory after all ops, native-30p | wasm 249.69 MB, browser RSS 1368.92 MB | wasm 74.84 MB, browser RSS 629.75 MB | RSS = whole browser tree, not heap |

### webkit 26.5

| op (ms, median) | HDDN | Foliark SDK | note |
|---|---|---|---|
| init cold (30.41 MB vs 5.41 MB downloaded) | 1,621 (p95 2,196, n=3) | 26.7 (p95 32.8, n=3) |  |
| init warm (assets cached) | 1,578 (p95 1,604, n=5) | 20.6 (p95 22.7, n=5) |  |
| native-30p: open + extract + render@1.35 every page | 1,124 (p95 1,174, n=5) | 223 (p95 225, n=3) | HDDN LOAD_PDF = parse + extract + page previews |
| native-30p: render@1.35 per page | 16.2 (p95 16.7, n=150) | 4.5 (p95 4.8, n=150) | HDDN encodes PNG, SDK returns RGBA |
| native-30p: first render after open (SDK) | n/a | 5.4 (p95 10.8, n=5) | includes one-time document scan |
| image-heavy-6p: open + extract + render@1.35 every page | 11,713 (p95 12,252, n=5) | 572 (p95 580, n=3) | HDDN LOAD_PDF = parse + extract + page previews |
| image-heavy-6p: render@1.35 per page | 6.2 (p95 6.7, n=20) | 96.8 (p95 98.7, n=30) | HDDN encodes PNG, SDK returns RGBA |
| image-heavy-6p: first render after open (SDK) | n/a | 96.5 (p95 109, n=5) | includes one-time document scan |
| native-30p: extract all pages | n/a | 33.6 (p95 86.7, n=5) | SDK only (no HDDN op without previews) |
| native-30p: search 'alpha' (SDK) / DETECT (HDDN) | 249 (p95 259, n=3) | 13.2 (p95 15.9, n=5) | NOT comparable: DETECT also runs PII, names, signatures |
| native-30p: paragraph prepare | 90.4 (p95 90.7, n=3) | 6 (p95 7.8, n=3) |  |
| native-30p: paragraph edit apply | 1,134 (p95 1,135, n=3) | 63.3 (p95 67.9, n=3) | HDDN also regenerates previews |
| native-30p: native export, 10 regions | 466 (p95 469, n=3) | 216 (p95 219, n=3) | out 213,136 B vs 322,769 B; same words removed |
| native-30p: raster export @2, 10 regions | 4,006 (p95 4,025, n=3) | 920 (p95 925, n=3) | out 9,075,784 B vs 13,360,325 B; different rasterizers |
| memory after all ops, native-30p | wasm 249.69 MB, browser RSS 234.26 MB | wasm 74.84 MB, browser RSS 252.41 MB | RSS = whole browser tree, not heap |

Per-page render, extraction composite, export breakdown (apply / prepare / export) and per-page medians are in the JSON.
Downloads: SDK cold init fetches 5,407,581 B identity (wasm 5,359,831 B raw, 2,050,728 B gzip -9 if the host compresses) vs HDDN 30.4 MB.

## NEW-63: decisions
Selected by the data above, not by guess. Before/after on the same bench (`tools/bench/sdk.mjs --extended`, chromium, same inputs; "before" wasm = HEAD d7a1a7f build):

| metric | before | after | change |
|---|---|---|---|
| wasm raw / gzip -9 | 5,312,423 / 2,035,689 | 5,359,831 / 2,050,728 | +0.9% / +0.7% |
| first renderPage right after open, native-30p | 9.5 ms | 4.9 ms | -48% |
| renderPage@1.35, image-heavy-6p (per page) | 117.6 ms | 106.4 to 109 ms | -8 to -10% |
| open+extract+render all pages, image-heavy-6p | 713 ms | 643 to 662 ms | -7 to -10% |
| native-30p render, extract, search, edit, native and raster export | unchanged within noise (4.2, 28, 9.6, 69, 245, 1010 ms) | | |

1. **Applied: no second whole-document stream decode at the first render** (`reader/preflight.rs`). `open` already walks and decodes every stream for the budgets (`info()`), and the first render walked them again. `info()` now marks the budget scan done. Same checks, same refusals; saves 4.6 ms of 9.5 ms on the 30-page text file and scales with stream bytes.
2. **Applied: opt-level 3 for the image path crates** `zune-jpeg zune-core moxcms pic-scale` (the hot symbols in a native `sample` profile of the image-heavy fixture). 8 to 10% faster render, +15 KB gzip. Marginal; kept because the size cost is under 1% and nothing else moves.
3. **Not fixed: image-heavy render is still about 100 ms/page (native aarch64 build: 75 ms)**, the largest remaining gap. Profile: 50% progressive-JPEG entropy decoding in `zune-jpeg` (the fixture's JPEGs come from PyMuPDF and are progressive; baseline JPEGs were not measured and are probably cheaper), 28% `pic-scale` vertical downscale, the rest ICC (moxcms). It is algorithmic (native is slow too), so build flags do not fix it. Next step is upstream: DCT-scaled decoding when the target is much smaller than the image, which needs a hayro/zune change out of scope here. A baseline-JPEG fixture should be added before quoting a number to customers.
4. **No change: wasm size.** `cargo tree -d`: `read-fonts` 0.43.3 (skrifa 0.46.2, pinned by vendored hayro-interpret `^0.46`) next to 0.45.1 (harfrust 0.14); the registry has no skrifa that uses 0.45, and harfrust 0.12 uses 0.41, so no compatible pin exists without patching a vendored crate and an API port. `fearless_simd` 0.7 (vello_common) vs 1.1, `miniz_oxide` 0.8.9 (png) vs 0.9.1 (flate2) are upstream-pinned. Embedded font payload is 320 KB raw, not the cost. `wasm-opt -O3/-Oz` and `-O1` were already compared in NEW-60 (gzip is what ships). No size claim beyond that; the size is 2.05 MB gzip (5.4 MB raw), up from 1.29 MB at the render spike because extract, edit, redact, sanitize and raster export landed since.
5. **No change: export latency (249 ms native, 1.0 s raster for 30 pages)** and `applyChanges` paragraph edit (69 ms). Both beat HDDN (399 ms, 4.1 s, 1.0 s) and are not first-order; native-export work is also being changed on main (NEW-80), so editing the writer here would conflict.
6. Tried and dropped: opt-level 3 for `miniz_oxide fdeflate png crc32fast adler2 simd-adler32` (flate decode and PNG encode): no measurable gain on export or raster export, cost size.

Regressions run after the change: `cargo test --workspace --no-fail-fast` (359 passed, 0 failed, 1 ignored; includes `strict_native`), `pnpm --filter @foliark/sdk test` (78 passed), `tests/smoke/run.mjs` chromium and webkit (ok, hang watchdog ok), `tests/package-consumers/run.sh` (6 builds ok).

## NEW-81: scanned and image-heavy pages
Data: `comparison.json` key `scans` (HDDN, SDK before, SDK after, fidelity). Fixtures: `tests/fixtures/generators/scans.py`, seven synthetic 300 dpi A4 scans, 3 pages each (baseline and progressive JPEG in RGB and gray, JBIG2 generic region, CCITT G4, Flate gray), byte-identical for both engines. Chromium 151, Apple M4, medians over 15 pages (HDDN) / 15 renders (SDK).

**Why the earlier HDDN numbers were wrong.** The 5 `GET_PAGE_PREVIEW` errors on `image-heavy-6p` (and on every scanned page) were not a size limit or a timeout: HDDN puts a scanned page in its OCR lane and `COMMIT_IMPORT` throws `incomplete-ocr` until OCR has run, so no document was committed and the preview failed with `errors.unknown`. The harness now commits with `allowIncompleteOcr: true` (OCR itself is not measured). With that HDDN renders all seven fixtures without error, and the old "SDK 109 ms vs HDDN 6 ms" compared against a failed call. HDDN `LOAD_PDF` takes about 12 s for a 3-page scan (OCR-lane classification); not compared here.

**What was measured first.** Native profile (macOS `sample`, symbolicated build): the downscale (pic-scale vertical pass, scalar in hayro without its `simd` feature) is 45% to 65% of a scanned page, then inflate (Flate) or JPEG entropy decoding. zune-jpeg full decode is 8 ms (baseline gray) to 26 ms (progressive RGB) for 8.7 Mpx. Scaled IDCT does not pay: zune-jpeg has no scaled output, and `jpeg-decoder`'s `scale()` (scratch bench) is slower than zune's full decode at every scale (baseline gray 10.6 ms at 1/2 vs 8.3 ms full; progressive RGB 42.9 vs 26.2), because entropy decoding dominates and cannot be skipped. The scale needed here is 1/3 (preview) or 1/1.4 (OCR), which scaled IDCT (1/2, 1/4, 1/8) could not use fully anyway.

**What was implemented (fork patch P17, `patches/hayro/README.md`).**
1. `InterpreterSettings::reduce_images`: the decoded gray or RGB samples are resampled once, with the renderer's own Hermite filter, to the drawn size (hayro's `target_dimension`), so the renderer does not resample again. Pixels match the full-size path (max channel difference 1, mean 0.00 to 0.13 at both scales); PyMuPDF differs from both by the same 0.02 to 3.1 mean. Images with alpha, masks, a transfer function or a decode that already changed size are left alone.
2. `InterpreterSettings::reduced_image_cache` and the engine's `ImageStore` (one per document, byte-bounded by the session cache budget, LRU): the reduced decode is kept per (object, reduced size, stencil). An image is only reduced from its second draw on (`ReducedImageCache::seen`), so a page drawn once, such as the OCR render, pays nothing.
3. `Document::render_page` (preview, SDK) uses both. `render_page_exact` (raster export burn-in, `export/raster_pixels.rs`) and `RasterImage::from_stream` (redaction pixel erasure) never reduce. Tests: `tests/render_images.rs` (first draw identical to exact, second within 2 levels and stored, third identical to second, native-size draw untouched) plus the existing raster and image redaction suites.
4. `opt-level = 3` for `hayro-interpret`, `hayro-jbig2`, `hayro-ccitt` (the reducer and the bilevel decoders run in wasm). wasm 5,566,051 B raw, 2,125,861 B gzip -9 (was 5,359,831 / 2,050,728: +3.9% / +3.7%).

Not done: a 1-bit-domain resample for CCITT and JBIG2 (the gray-domain reducer already beats HDDN 4.2x and 1.4x at 1.35), and a faster first draw. In wasm the reducer is not faster than pic-scale's SIMD fixed-point path, so the second draw costs 3 to 9 ms more than a plain draw, once, in exchange for 5x to 30x on every later draw of the same page and scale.


#### Scale 1.35, ms per page, median, chromium 151.0.7922.34

| fixture | HDDN (PyMuPDF + PNG) | SDK before | SDK after, 1st render | SDK after, 2nd | SDK after, 3rd and later | 1st vs HDDN |
|---|---|---|---|---|---|---|
| jpeg-baseline-rgb | 73.7 | 40.5 | 41.8 | 48.9 | 2.2 | 1.8x faster |
| jpeg-baseline-gray | 66.7 | 17.8 | 18.2 | 21.1 | 1.5 | 3.7x faster |
| jpeg-progressive-rgb | 76 | 80.3 | 82.5 | 91 | 2.1 | 0.9x faster |
| jpeg-progressive-gray | 64.6 | 50.7 | 51.3 | 54.3 | 1.3 | 1.3x faster |
| jbig2 | 29.3 | 21.6 | 21.4 | 24.2 | 1.3 | 1.4x faster |
| ccitt-g4 | 28.4 | 6.7 | 6.7 | 9.6 | 1.2 | 4.2x faster |
| flate-gray | 94.4 | 16.8 | 16.7 | 19.5 | 1.2 | 5.7x faster |

#### Scale 3 (HDDN OCR scale), ms per page, median, chromium 151.0.7922.34

| fixture | HDDN (PyMuPDF + PNG) | SDK before | SDK after, 1st render | SDK after, 2nd | SDK after, 3rd and later | 1st vs HDDN |
|---|---|---|---|---|---|---|
| jpeg-baseline-rgb | 245.1 | 54.7 | 57.4 | 77.5 | 10.3 | 4.3x faster |
| jpeg-baseline-gray | 253.9 | 27.2 | 27.7 | 33.3 | 6.4 | 9.2x faster |
| jpeg-progressive-rgb | 254 | 94.1 | 97.8 | 118 | 10.2 | 2.6x faster |
| jpeg-progressive-gray | 246.2 | 59.9 | 61.3 | 66.8 | 6.1 | 4.0x faster |
| jbig2 | 90.8 | 31 | 30.9 | 37 | 6.1 | 2.9x faster |
| ccitt-g4 | 88.4 | 16.1 | 16.4 | 22.4 | 6.3 | 5.4x faster |
| flate-gray | 571 | 26.3 | 26.4 | 32.5 | 6.1 | 21.6x faster |


Reading it: the first render is unchanged (within noise, by design). The SDK is faster than HDDN on every fixture and scale except progressive RGB at scale 1.35 (82.5 vs 76 ms, 0.9x: zune-jpeg progressive entropy decoding, not addressable here). A re-render (3rd and later draw) takes 1.2 to 2.2 ms at 1.35 and 6 to 10 ms at the OCR scale. The store holds 16 MiB per document by default, so only the most recent pages and scales stay (one RGB page at scale 3 is 13.5 MB).
