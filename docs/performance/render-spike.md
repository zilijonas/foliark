# Render performance spike (NEW-63)

Goal: `renderPage` at scale 1.35 on the 30-page native-text fixture (`native-30p`) from 129 ms/page to PyMuPDF parity (15.9 ms).
Result: **4.0 ms/page in Chromium, 4.1 in WebKit**, with a smaller wasm. Pixels are bit-identical across SIMD and non-SIMD builds.

## Method
`tools/bench/render-spike.mjs`: warm, 1 discarded round + N=5 rounds over every page, median, `renderPage` through the SDK worker (end to end).
Phase split on the main thread against raw wasm exports. Chromium 151 headless, Apple M4. Each variant is a clean rebuild
(`scripts/build-wasm.sh`; env `RUSTFLAGS`, `CARGO_PROFILE_RELEASE_OPT_LEVEL`). Native reference: `cargo run --release -p foliark-engine --example render_bench`.

## Results (ms/page median, wasm bytes raw / gzip -9)
| variant | native-30p | native-small (1 page) | raw | gzip |
|---|---|---|---|---|
| baseline: opt "s", no simd128 | 130.1 | 3.53 | 4,878,707 | 1,446,933 |
| simd128, opt "s" | 23.3 | 1.65 | 3,484,059 | 1,271,811 |
| no simd128, opt 3 | 9.6 | 1.11 | 3,719,595 | 1,403,733 |
| simd128, opt 2 | 4.19 | 0.73 | 3,529,414 | 1,361,494 |
| simd128, opt 3 | 4.12 | 0.82 | 3,638,319 | 1,384,828 |
| simd128, "s" + hot crates at 3 | 4.32 | 0.80 | 3,430,675 | 1,294,114 |
| **final: same + `takeData` (no Rust-side clone)** | **3.98** | **0.75** | **3,430,716** | **1,294,197** |
| final in WebKit 26.5 | 4.06 | 1.46 | same | same |
| final without simd128 (fallback) | 8.95 | 1.09 | 3,550,865 | 1,327,471 |
| native cargo, same hayro (aarch64 NEON) | 3.5 | n/a | n/a | n/a |

## Findings
1. Time is almost all in hayro raster, not plumbing. Baseline split: wasm `renderPage` 131.5 ms, RGBA copy out 0.3-0.45 ms,
   end-to-end 130 ms. postMessage with transfer is within noise (end-to-end minus wasm call is below 0.5 ms).
2. Two multiplicative causes: (a) `opt-level = "s"` on the hot loops, 130 to ~9.6 ms once built at 3 even with no SIMD, and
   (b) vello_cpu/fearless_simd only select the wasm SIMD path when `simd128` is a compile-time target feature
   (`fearless_simd` `Level` is cfg-gated; there is no runtime detection on wasm), 9.6 to 4.1 ms. SIMD alone at "s" gets only 23 ms
   because the SIMD kernels are not inlined at "s". Both are needed.
3. Size goes down, not up: SIMD removes the scalar fallback kernels (-1.4 MB raw). Per-package opt 3 for
   `hayro vello_cpu vello_common fearless_simd kurbo` costs +0 to +0.2 MB vs all-"s" (smaller than opt 2/3 globally) and is as fast.
4. Pixel check: pages 0, 7, 19 of native-30p (10.9 M bytes) are byte-identical between simd128 and non-simd builds
   (max diff 0). `tests/smoke` passes; `cargo test` and `clippy -D warnings` pass.
5. Per-call re-init: `render_page` builds `RenderCache::new()` each call, so fonts and glyph outlines are re-derived per page.
   Native A/B (fresh vs one shared cache per document) on `native-30p`: 3.55 vs 3.49 ms (1.7%). Not worth it here: the cache
   borrows the `Pdf` (`RenderCache<'a>`), so a per-document cache needs a self-referential struct (unsafe or `self_cell`).
   Revisit with a font-heavy fixture (embedded subsets, CID fonts), where outline reuse should matter more.
6. Document is not reparsed per call (`Pdf` lives in the handle table). The old `data` getter cloned the Vec in Rust before
   wasm-bindgen copied it again; `takeData` moves it (0.28 to 0.20 ms/page, about 2% of the render).
7. hayro knobs: `RenderSettings` only has `force_image_interpolation`; `vello_cpu` already uses the `u8_pipeline` (speed);
   `multithreading` is not available on wasm; hayro's own `simd` feature only enables native `pic-scale` image-resize kernels (no wasm target).
   Not touched.
8. wasm overhead vs native is ~13% (3.98 vs 3.5 ms). hayro itself is fast; the baseline was a build-flag problem.

## Recommendation (applied in this commit)
- `.cargo/config.toml`: `[target.wasm32-unknown-unknown] rustflags = ["-C", "target-feature=+simd128"]` (applies to `scripts/build-wasm.sh`
  and CI; a user `RUSTFLAGS` env var would override it, so keep it unset). Needs wasm SIMD: Chrome 91, Firefox 89, Safari 16.4.
- `Cargo.toml`: keep `[profile.release] opt-level = "s"`, add `[profile.release.package.{hayro,vello_cpu,vello_common,fearless_simd,kurbo}] opt-level = 3`.
- `foliark-wasm`: `RenderedPage.takeData()` replaces the cloning `data` getter; `packages/sdk/src/worker.ts` updated.
- Not applied: cross-call `RenderCache` (finding 5), a no-SIMD fallback artifact (only if a supported browser lacks wasm SIMD; it would be 9 ms/page and +3 MB to host).

## Can hayro reach 16 ms/page at 1.35?
Already 4x under it (4.0 ms), also in WebKit. No residual gap on this fixture. Not measured: image-heavy pages
(decode and resample, hayro `simd`/`pic-scale` has no wasm path) and font-heavy documents; re-run `tools/bench/run.mjs` for those before claiming wider parity.
If a heavy page ever exceeds budget: render in tiles or progressively at low scale first, or upstream a wasm path for `pic-scale`.
