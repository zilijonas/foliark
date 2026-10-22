# Foliark support matrix and regression budgets (NEW-65)

Frozen 2026-10-22 for `@foliark/sdk` 0.1.0 (private, unreleased) from receipts already in this repository. Nothing here is a new claim: each row names the
document or test that produced it. Machine-readable budgets: `tools/bench/budgets.json`, checked by `tools/bench/check-budgets.mjs`.

Words used with one meaning:
- **supported**: exercised by the cited receipt on the cited corpus. It says nothing about files outside that corpus.
- **refused (CODE)**: the engine returns that typed `FoliarkError` and produces no output (spec 0.1). Not a bug; the caller decides what to do.
- **not proven**: no receipt covers it. It may work. Do not rely on it.
- **not established**: needs a device or access this project does not have. Treated as unsupported until measured.

## 1. Browsers and devices

| Target | Status | Evidence | Gaps |
|---|---|---|---|
| Chromium 151.0.7922.34, desktop, macOS arm64 (Playwright headless) | supported | perf budgets (section 5); `pnpm smoke chromium`; `tests/package-consumers/run.sh` (Vite, esbuild, no bundler, at `/` and `/app/`); `tests/browser/run.sh` (CSP, offline, full flow) | one machine (Apple M4, 24 GB) |
| WebKit 26.5, desktop engine (Playwright) | supported as an engine, **not as Safari** | perf budgets (section 5); `pnpm smoke webkit`; `tests/browser/run.sh` | Playwright WebKit is not Safari evidence. Under Playwright, `page.reload()` while offline with a service worker fails, so the offline spec stops the origin instead (`docs/deployment.md`) |
| Firefox 153.0, desktop (Playwright) | functional only | `tests/browser/run.sh`: CSP policy, prepared offline, only-own-origin requests, missing-asset errors, full open/extract/render/redact/export flow | no perf numbers, no budgets, no `pnpm smoke firefox`, package consumers run in chromium only |
| Safari on macOS (real) | not established | none | needs a real Safari run |
| Safari on iPhone and iPad (real devices) | **not established, pending NEW-64 (owner device access NEW-22)** | none. No iOS Simulator on the build Mac | memory ceiling, worker lifetime, wasm limits on iOS are unknown; mobile default `Limits` stay undecided (spec 17.4) |
| Chrome on Android (real device) | **not established, pending NEW-64 (owner device access NEW-22)** | none. No Android emulator on the build Mac | same as above |
| Windows, Linux, other Chromium-based browsers | not established | none | |
| Browser version floors | not measured | only the versions above were run | The wasm needs SIMD, bulk memory, sign extension, mutable globals, reference types and multivalue (`scripts/build-wasm.sh`). No older browser was tested, so no minimum version is promised |

Host requirements, each tested in the browser specs: static files only, `application/wasm`, CSP `script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'`
(without `'wasm-unsafe-eval'` init fails `ASSET_LOAD_FAILED`), no COOP/COEP, no SharedArrayBuffer, no network after the files are cached. Details: `docs/deployment.md`.

## 2. Features

| Feature | Status | Evidence | Typed refusals and warnings | Not proven |
|---|---|---|---|---|
| Open | supported | 35 manifest fixtures: 25 open and round-trip, 8 refused, `docs/evidence/native-roundtrip.md` | `MALFORMED_PDF`, `REPAIRED_PDF` (unless `allowRepaired`), `DECODE_FAILED`, `LIMIT_EXCEEDED`, `PASSWORD_REQUIRED`, `PASSWORD_INCORRECT`, `ENCRYPTION_UNSUPPORTED` | files unlike the synthetic corpus |
| Encrypted input | supported: RC4-40/128, AES-128/256 with the password; output is never encrypted | `native-roundtrip.md`, `strict-native.md` (4 encrypted fixtures) | `PASSWORD_*`, `ENCRYPTION_UNSUPPORTED` for unknown handlers | encrypted raster fidelity (5 fixtures judged by `objects.py` only) |
| Extract text | supported | HDDN parity: 21 fixtures against PyMuPDF 1.26.5, equal text, offsets, keys, worst box deviation 0.211 pt (spec allows 0.75), `docs/integration/hddn-adapter.md` | none beyond open | CID fonts without ToUnicode, vertical and RTL text |
| Search, hit test, select | supported on the multi-column Unicode fixture (diacritic folding, column drag stays on its line) | `hddn-adapter.md` (NEW-67) | `INVALID_ARGUMENT` | regex and hyphenation-aware search are absent by design (spec 16); not wired into HDDN's UI |
| Render | supported with warnings | `native-roundtrip.md` (0 channel values off by more than 2/255 between input and output renders); reduced vs exact decode max diff 1 (`docs/performance/comparison.md`) | warnings `FONT_SUBSTITUTED`, `IMAGE_DECODE_FAILED`, `ANNOTATION_APPEARANCE_MISSING`, `CMAP_MISSING` (page is `complete:false`); `LIMIT_EXCEEDED` over `maxRenderPixels` | pixel accuracy against a second renderer on real documents (PyMuPDF differs by mean 0.02 to 3.1 levels on the scan fixtures) |
| Predefined CMaps (legacy CJK encodings) | supported (pack in the package) | `cmaps/` 63 Adobe files; fixture `generated/cjk-predefined-cmaps.pdf`: extract equals PyMuPDF, render has no `CMAP_MISSING`, redaction + native export judged by `objects.py`/PyMuPDF (`crates/foliark-engine/tests/cmaps.rs`), packed-package browser specs in 3 engines (`tests/browser/cmaps.spec.ts`) | `ASSET_LOAD_FAILED` at `open` for a missing or corrupt pack file; `CMAP_MISSING` for a name outside the predefined set; the pack must be served or copied by the caller (`docs/deployment.md`) | text redaction/edit on mixed-length encodings (RKSJ, EUC, UTF16): `FONT_UNSUPPORTED`; CJK glyphs of a non-embedded font are drawn missing |
| Paragraph edit | supported for horizontal, unrotated, left-to-right text in a registered or reusable font | `docs/adr/0003`, `0004`, `0005`; acceptance scenario E over 80 cases; HDDN adapter NEW-68 | `EDIT_NO_FONT`, `EDIT_NO_FIT`, `EDIT_NO_STYLE`, `EDIT_UNSUPPORTED_SCRIPT` (non-BMP, RTL), `EDIT_ROTATED_TEXT`, `FONT_UNSUPPORTED`, `STALE_REVISION` | CFF/Type1 embedded faces, RTL, CJK, vertical, auto-fit (spec 16) |
| Native export (sanitized rewrite) | supported | `strict-native.md`: 39 fixtures, 31 supported + 8 refused, 0 skipped, independent oracles, negative controls caught; acceptance 80 cases x 6 scenarios | `FORM_APPEARANCE_MISSING`, `XFA_UNSUPPORTED`, `OPTIONAL_CONTENT_UNSUPPORTED`, `SIGNED_INPUT` (policy `refuse`), `REDACT_*`, `STALE_PLAN`, `PLAN_CONSUMED` | what sections 3 and 4 list |
| Raster export (image-only, redactions burned in) | supported; needs `policy.raster.acknowledgeLosses` | acceptance: 143 SAFE + 17 refused raster results; `comparison.md` (30 pages, 10 regions, 0 words in the regions) | `CONSENT_REQUIRED`, `RENDER_INCOMPLETE {pages}` (3 acceptance cases, annotation-comment pinned by NEW-56) | encrypted raster fidelity; edge pixels touching a region are checked with a 0.26 pt inset |
| Cancellation | supported before an operation starts; results of aborted operations are atomic | acceptance: 27 attempts, finals byte-equal to a clean run | `CANCELLED` | abort while an operation runs through the packed SDK (only 3 pre-aborted attempts ended `CANCELLED`) |
| Worker recovery | supported | `docs/deployment.md`, smoke hang test (120 MB page, watchdog resets the worker) | `WORKER_RESET`, `DOCUMENT_GONE`, `OUT_OF_MEMORY` | real out-of-memory on a phone |

## 3. Native redaction by content class

Native export is: edits, flatten, redact, sanitize, rewrite, verify (spec 5). "Supported" means the independent oracles found no leftover and no collateral outside the declared regions on the cited corpus.

| Content under a region | Status | Evidence | Typed refusal or disclosure |
|---|---|---|---|
| Text: visible, invisible (Tr 3/7), Type3, inside Forms, flattened annotations and widgets | supported | `strict-native.md` (secret in every `mustRemove` string removed from bytes, streams and both extractors); acceptance cases tj-kerning, split-ops, hex-escaped, quote-ops, render-modes, ligature, rotated-text-30, type3-glyphs, form-both-places | none |
| Text inside a tiling-pattern cell | refused | acceptance case pattern-text-cell | `REDACT_PATTERN_UNSUPPORTED {pages}` (raster export of the same page works) |
| Images: DCT and Flate, XObject, inline, inside Forms, SMask | supported; re-encoded to 8-bit Flate | `strict-native.md` (mixed-regions, jpeg-payload-secret, ocr-over-image); acceptance image-inline, image-smask-only | `IMAGE_REENCODED`; shared images with untouched occurrences are cloned and listed in `sharedSurvivors` |
| Images: JPX, JBIG2, CCITT payloads | **not proven** (kept byte for byte when untouched, no control for redaction) | `strict-native.md`, `security-acceptance.md` | |
| Image inside a tiling-pattern cell | refused | acceptance case pattern-image-cell | `REDACT_PATTERN_UNSUPPORTED {pages}` |
| Vectors | supported: axis-aligned fill-only crossing paths are cut exactly; any other crossing path is removed whole | `strict-native.md` (shared-vector, mixed-regions); spec 5.4 | `VECTOR_COLLATERAL` with bounds in `report.redaction.collateral`. Exact curve and stroke subtraction is absent (spec 16) |
| Form XObjects | supported: occurrences needing change are cloned, shared ones are never edited in place | form-reuse-matrices, form-both-places, shared-stream-pages | none |
| Soft-mask groups | refused | acceptance case smask-group-text | `INVALID_ARGUMENT {feature: redactionSoftMaskGroup}` (spec 5.3 plans support; raster export works) |
| Fonts touched by removal | TrueType glyphs emptied, ToUnicode rebuilt; **CFF and Type1 programs are kept** | `strict-native.md` (font-payload-secret); spec 5.6 | `FONT_GLYPH_INVENTORY_RETAINED`, names in `report.kept.fontsWithGlyphInventory`. Whether a retained CFF/Type1 inventory leaks removed glyph shapes is not proven |
| Hidden text: OCG-off, invisible, white-on-white, clipped out, covered | supported when under a region (default policy keeps invisible text elsewhere) | acceptance cases ocg-off-in-form, invisible-tr3, white-and-covered, clipped-text | occlusion-aware detection outside regions is absent (spec 16) |
| Unused fonts, Forms, images reachable from Resources | removed from every Resources dictionary | `docs/adr/0007-unused-resources.md`; acceptance unused-xobject, unused-font | `SanitizedItem unusedResources` |
| Verification | in-engine strict reparse plus zero re-extracted clusters in each region | spec 5.7; acceptance negative controls 109/109 caught | `REDACT_VERIFY_FAILED` (a bug guard; no fixture triggers it). It is not a privacy certificate |

## 4. Sanitization classes (native export; raster export drops all by construction)

Each class is removed and reported, or refused. Evidence is `strict-native.md` (manifest, oracles `objects.py`, `fidelity.py`, PyMuPDF, pypdf, pikepdf) plus the acceptance matrix (41 adversarial cases) unless stated.

| Class | Status | Report |
|---|---|---|
| Info and XMP metadata, PieceInfo, private keys | supported (removed) | `METADATA_REMOVED` |
| Embedded files, /AF | supported (removed) | `ATTACHMENTS_REMOVED` |
| JavaScript, OpenAction, AA, all actions | supported (removed) | `SCRIPTS_REMOVED` |
| Annotations | supported (flatten with appearance, or remove) | `ANNOTATIONS_FLATTENED`, `ANNOTATIONS_REMOVED`, `LINKS_REMOVED` |
| AcroForm and widgets | supported (appearances flattened, values removed); a value without a usable appearance is refused | `FORMS_FLATTENED`; `FORM_APPEARANCE_MISSING {fields}`, `XFA_UNSUPPORTED` (engine tests `sanitize_interactive.rs`; no manifest fixture, no independent oracle) |
| Optional content | supported (default configuration flattened, `/VE` And/Not evaluated, hidden layers removed) | `LAYERS_FLATTENED`; `OPTIONAL_CONTENT_UNSUPPORTED` when visibility cannot be evaluated (no manifest fixture) |
| Thumbnails, outlines, page labels, structure tree, ActualText/Alt, output intents, viewer preferences | supported (removed) | `thumbnails`, `OUTLINES_REMOVED`, `PAGE_LABELS_REMOVED`, `ACCESSIBILITY_REMOVED`, `PDFA_CLAIM_REMOVED` |
| Prior revisions, orphan objects, trailing bytes, unreachable object-stream entries | supported (absent by construction) | none needed |
| Encryption | supported (decrypted, never re-encrypted) | `ENCRYPTION_REMOVED` |
| Signatures | stripped with disclosure, or refused by policy (engine tests `sanitize_interactive.rs`, `raster_plans.rs`; no independent oracle) | `SIGNATURES_INVALIDATED`; `SIGNED_INPUT` |
| Content outside the crop box | supported (removed) | `offPage` |
| Invisible text | kept by default (so detection sees it), `remove` by policy, `keepOcr` keeps only OCR over drawn images (ADR 0008) | `report.kept.invisibleTextClusters`, `removed.invisibleText` |

Not proven for sanitization: content inside an ICC profile's declared size, Type3 CharProcs drawing with other fonts, inline images in the image oracle, secrets that are not ASCII markers, files that are not like the 80 synthetic cases (`security-acceptance.md`, "Not proven").

## 5. Measured cost (desktop, regression budgets)

Source: `tools/bench/sdk.mjs --extended` and `tools/bench/scans-sdk.mjs` on the build commit named in `budgets.json` (wasm sha256 in the file). Apple M4 24 GB, headless, localhost, identity encoding, other agents' jobs may have been running, so differences under about 10% are noise.
Medians in ms, best of the runs listed in `budgets.json`; "budget" is the frozen limit. The full list (76 timing and size budgets, both browsers) is in `budgets.json`.

| Operation (input) | Chromium measured / budget | WebKit measured / budget |
|---|---|---|
| wasm raw / gzip -9 | 5,567,725 B / 2,126,019 B (budget 5,846,112 / 2,232,320) | same file |
| cold init, bytes downloaded | 5,615,475 B (budget 5,896,249) | same |
| cold init (fresh profile) | 22 / 36 | 26.7 / 44 |
| warm init | 12.9 / 23 | 21.9 / 36 |
| open + extract + render@1.35 every page (30-page text file) | 223.7 / 339 | 235.9 / 357 |
| render@1.35 per page (30-page text file) | 4.2 / 10 | 4.8 / 11 |
| extract text, 30 pages | 32.5 / 52 | 37.3 / 59 |
| search (30 pages, 840 hits) | 10.3 / 19 | 13.8 / 24 |
| paragraph edit, prepare + apply | 5.6 + 65 / 12 + 101 | 6.2 + 63.3 / 13 + 98 |
| native export, 10 regions, 30 pages | 233 / 353 | 203.4 / 309 |
| raster export @2, 10 regions, 30 pages | 1,026.5 / 1,543 | 917.4 / 1,380 |
| image-heavy 6-page file: first render of a page / cached repeat | 110.2 / 169 and 2.2 / 7 | 98.9 / 152 and 2.7 / 8 |
| scanned 300 dpi A4, first render @1.35: JPEG baseline RGB / progressive RGB / JBIG2 / CCITT G4 / Flate gray | 41.6 / 80.7 / 21.3 / 6.9 / 16.6 | 42.7 / 87 / 24 / 8.1 / 16.9 |
| scanned page, repeat render @1.35 | 1.2 to 2.0 (budget 5 to 6) | 1.3 to 2.2 (budget 5 to 7) |
| wasm linear memory after all operations (30 pages) | 74,711,040 B (budget 93,388,800) | same |

How the budgets were set: each timing budget is `ceil(measured x 1.5 + 3 ms)`, each size budget `measured x 1.05`, memory `measured x 1.25`. The two chromium runs differed by at most 10% on medians above 5 ms, so 50%
headroom flags a real slowdown without tripping on a busy machine. `check-budgets.mjs` compares the best median across the fresh files you give it (rerun to isolate noise), exits 1 on any
regression and on any budgeted number that was not measured, and reports browsers without a fresh file as skipped. `tools/bench/check-budgets.test.mjs` proves it fails on an injected 2x slowdown and on injected wasm growth and passes on noise.
Whole-browser RSS is recorded in `docs/performance/comparison.json` but not budgeted (process tree, noisy; WebKit's value misses helper processes).

Known performance limits, from `docs/performance/comparison.md`: progressive-JPEG RGB scans at scale 1.35 are not faster than HDDN (80.7 ms vs 76 ms; entropy decoding dominates, no upstream scaled decode); the first draw of an image-heavy page costs about 100 ms;
a re-render within the 16 MiB per-document image cache is cheap, an evicted one is not. These numbers are from synthetic fixtures on one machine and say nothing about phones.

## 6. Limits and what is not established

- Mobile: every iPhone, iPad and Android row above, mobile default `Limits`, memory ceilings and thermal behaviour: not established (NEW-64, NEW-22).
- Documents: the corpus is 80 small synthetic and HDDN cases (39 manifest fixtures plus 41 adversarial) and 7 synthetic scan fixtures. No real-world document set has been run, and the speed numbers are not a promise for large or unusual files.
- Default limits (`docs/spec/v1.md` section 9): 64 MiB input, 1000 pages, 50 Mpx per render, 256 MiB output, 4 open documents. Over a limit is `LIMIT_EXCEEDED` with no output.
- Independent proof used one PDF renderer (MuPDF) for pixels and PyMuPDF/pypdf/pikepdf for structure; none is a certificate that a redacted file is private.
- No universal speed or security claim is made. Comparisons with HDDN are in `docs/performance/comparison.md` with their caveats.

## 7. Refreshing this file

When the engine or its dependencies change: rebuild (`scripts/build-wasm.sh`, `pnpm build:sdk`), rerun `node tools/bench/sdk.mjs --extended` and `node tools/bench/scans-sdk.mjs` per browser through the memory queue,
run `node tools/bench/check-budgets.mjs <files>`, and only after a deliberate decision regenerate the budgets with `--derive`. A new real device or browser becomes a row only with its own receipt.
