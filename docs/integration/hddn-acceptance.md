# HDDN replacement and rollback acceptance (NEW-76)

Proof that HDDN runs on the pinned private Foliark candidate **rc2** and no longer has a Python path, and that the pre-Foliark build can be restored. rc2 replaces `b47dafd9` (NEW-82 and NEW-83 fixes); every number below was re-measured on rc2.
Local only: nothing was pushed, deployed or published. HDDN worktree `~/ai/repos/hddn-foliark`, branch `foliark/integration`. The `~/ai/repos/hddn` checkout was not touched.

## Pinned inputs
| | |
|---|---|
| package | `dist-pkg/foliark-sdk-0.1.0.tgz` rc2 sha256 `4ead7f33334259f020d25ebac2c1abae3739fc03b797b039a6c60064a46c4af4`, 2,224,913 B, built from foliark `ca63d50`, recorded in `docs/release/candidate.md` (foliark commit `0b2c947`). Checked with `shasum -a 256` before install |
| installed integrity | `pnpm-lock.yaml` `sha512-TPDfk9UiLtzDISlE0pi/86/7uJCZSTCl2xmDgk2g0+X8RREfes9lMVba03wp24R2s9jZncp4e7bj4hDNEQ7r0Q==`; recomputed independently with `openssl dgst -sha512 -binary \| base64`, identical. Installed wasm sha256 `4e0b704521a1aa93...` (candidate record). `package.json` already pointed at the same tarball path, so only the lockfile integrity changed |
| HDDN commits | `57e9d8f` pins rc2 (lockfile only), on top of `b66b5f7` (NEW-82 adapter fixes), `3841e17` (e2e and harness), `f07d35a` (rc1 pin). Branch `foliark/integration`, local only |
| rollback ref | `codex/hddn-export-buffer` `2fe0f9a`, built in the scratch worktree `~/ai/repos/foliark/.worktrees/hddn-2fe0f9a` |
| machine | Apple M4 24 GB, macOS arm64, Chromium 151.0.7922.34, Firefox 153, WebKit 26.5 (Playwright; WebKit is not Safari) |

## Results on the pinned build
| Suite | Command (all through `~/ai/bin/memory-queue`) | Result |
|---|---|---|
| HDDN validate | `pnpm run validate` (lint, format, check, styles, tooling, 62 test files, build) | exit 0, 783 tests, build verification passed |
| Real-browser UI e2e, Chromium | `E2E_BROWSER=chromium E2E_EXPORT_DIR=<dir> pnpm run test:e2e:foliark` | exit 0, 25/25 |
| Real-browser UI e2e, Firefox | `E2E_BROWSER=firefox E2E_EXPORT_DIR=<dir> pnpm run test:e2e:foliark` | exit 0, 25/25 |
| Real-browser UI e2e, WebKit | `E2E_BROWSER=webkit E2E_EXPORT_DIR=<dir> pnpm run test:e2e:foliark` | exit 0, 25/25 |
| Built-network scan (same runs) | every URL the page asked for | retired-engine (Pyodide, PyMuPDF, MuPDF, wheel) requests: 0 in all three; wasm = Foliark + Tesseract only (124, 124, 225 distinct URLs) |
| Independent export integrity | `tests/oracles/.venv/bin/python tools/hddn-acceptance/check-exports.py <dir> tests/foliark/fixtures` on the files each browser saved through the UI | exit 0 for Chromium, Firefox and WebKit downloads: native PASS (6 regions, control flagged 11), edit+redact PASS (4 regions, control flagged 7), raster PASS (1 region, control flagged 4) |
| Isolated actual-worker harness (docker) | `pnpm run test:pdf:isolated` | exit 0: 21 fixtures x 2 modes, 21/21 with 0 findings; privacy expectations unchanged since NEW-82 |
| Rollback | `node tools/hddn-acceptance/rollback.mjs HDDN_DIST ROLLBACK_DIST multicolumn-unicode.pdf` | exit 0 (phases 1 and 2); offline re-preparation after rollback not established, see Rollback |

The e2e gained one case (25th): `no sanitize-only path`. It also keeps exported files under `E2E_EXPORT_DIR` and records the source URL of every console error.

### Export integrity through the HDDN UI
`E2E_EXPORT_DIR=<dir> pnpm run test:e2e:foliark` keeps every file the UI wrote. `check-exports.py` judges them with foliark's own oracles (`objects.py` privacy, `fidelity.py` appearance and text outside the changed regions),
which never import foliark, and runs the same privacy check on the INPUT as a negative control (it must flag the secret there, or the case fails).
| Flow (UI case) | Input | Checked | Result |
|---|---|---|---|
| native redaction | `multicolumn-unicode` | email, phone, name gone from decoded text (pypdf + PyMuPDF), content streams, strings; every other word and pixel unchanged outside the 6 declared word regions | PASS (Chromium files) |
| edit + redact | `edit-paragraphs` | old paragraph text and the confirmed name gone; edit and redaction regions declared; the rest unchanged | PASS (Chromium files) |
| image-only (raster, after consent) | `pattern-secret` | no text layer (`RASTER_TEXT`), email region burned, rest within the raster tolerance (threshold 64, 1% of pixels) | PASS (Chromium files) |
| sanitize-only | none | **no UI path**: with every marking dismissed HDDN refuses the export ("Confirm or add at least one", no file kept); case `no sanitize-only path` asserts that. Sanitize-only export is proven at SDK level (`docs/evidence/security-acceptance.md`, scenario sanitize, 80 cases) | not a UI flow |
Oracle findings that were test-side, not product defects, and what was changed: (1) the short needle `34567` also matches a UTF-16 character range inside an embedded TrueType program (`objects.py` STREAM on font bytes), so the check uses the whole phone string; (2) "Confirm all markings" also confirms the auto-detected name, so the name is a declared redaction; (3) raster regions are the real word boxes, not a sampled strip. The extracted-text, pypdf and PyMuPDF checks were clean throughout.

### Isolated worker harness findings (8 fixtures, triaged in NEW-82; no privacy expectation weakened; 21/21 clean again on rc2)
`pnpm run test:pdf:isolated` exits 0 on rc2 (21 fixtures x 2 modes, all 0 findings), as it did on the NEW-82 interim build. Every change and its reason is in HDDN `tests/pdf/README.md` ("Isolated worker harness on Foliark").
| Fixture | Verdict | What changed |
|---|---|---|
| hidden-ocr-text | adapter policy | native export uses `invisibleText: 'keepOcr'` (ADR 0008, NEW-84): the secret over blank paper is removed, a scan's OCR word over a drawn picture stays searchable (fixture now holds `KEEP_SCANNED_OCR_WORD`, `mustKeepText`); notice `invisibleRemoved` counts what went |
| shared-image, shared-vector | adapter policy | native export stops (`native-redaction-unsupported`, `errors.nativeSharedContentPages`) while a marked graphic is still drawn on an unmarked page; marking that page, or image-only, exports. Engine spec 5.2 unchanged |
| oversized-image | by spec | 10^10-pixel bomb; typed refusal is the fixture's required outcome |
| compressed-stream | by spec, plus a limit retuned | the 8 MiB no-op content stream stays refused (bomb). `maxDecodeRatio` no longer applies to image XObjects (a blank 150 dpi scan was refused at 1026:1); regression in `resource_limits.rs` |
| annotation-comment | adapter policy | image-only retries with `annotations: 'remove'` on `RENDER_INCOMPLETE` |
| actualtext, unicode-glyph-widths | check tolerance | image-only pixel comparison forgives a 2 px rasterizer shift; privacy checks unchanged |

### Mobile, offline, save and cancel
- Real mobile: **not established, pending NEW-64 (device access NEW-22).** No claim. Playwright WebKit is not Safari.
- Offline, prepared: e2e `offline lifecycle` (3 browsers): "Download tools" caches the engine wasm, edit fonts and OCR data; the origin is then stopped and a PDF opens and is read; no document text lands in any cache. Cold (not prepared, engine unreachable): e2e `engine files unreachable` shows a clear message and a retry works once the files return.
- Save and cancel: e2e `save cancelled`, `write failure` (partial write aborted, no download fallback), `no save picker` (browser download, never claimed as saved), `repeated clicks` (one picker, one file), `cancel import`, a refused native export (empty file removed, image-only only after consent). Cancel buttons for import and export exist; there is none for paragraph edits (none exists in HDDN).

## Operation map: every HDDN worker request
Worker protocol: `src/types/worker.ts` (HDDN). Baseline: `docs/research/hddn-baseline.md` section 2. "Refused" means the user sees a typed, localized failure and nothing is retried on another engine.
| HDDN request | Foliark API used (via `src/workers/lib/foliark*.ts`) | Evidence | Supported / refused boundary |
|---|---|---|---|
| INIT | `createInlineFoliark({wasmUrl, limits: maxInputBytes 25 MiB, maxPages 30, maxOpenDocuments 2})` | every e2e case; `engine files unreachable` | wasm unreachable: `errors.engineUnavailable`, failed start not cached. No Pyodide boot |
| PRELOAD_TOOLING, TOOLING_STATUS | none (OCR data and cache probe; wasm is in the offline shell) | `offline lifecycle`, harness preload | English OCR data is bundled; other languages come from the configured CDN only when the user asks |
| LOAD_PDF | `open(bytes,{password,allowRepaired:false})`, `pages()`, `extractText(i)` | `foliark.test.ts` parity on 21 fixtures vs PyMuPDF 1.26.5 (equal text, offsets, keys; worst box 0.211 pt); `router.foliark.test.ts`; e2e native, scanned, mixed | refused: `MALFORMED_PDF`, `REPAIRED_PDF`, `DECODE_FAILED`, `ENCRYPTION_UNSUPPORTED` as `malformed-pdf`; `PASSWORD_REQUIRED/INCORRECT`; page limit as `too-many-pages`; other limits as `resource-exhausted`. `oversized-image` and `compressed-stream` refused where PyMuPDF loaded them |
| CONTINUE_OCR | `renderPage(i,{scale 3})` RGBA to PNG, Tesseract in the worker | e2e scanned, mixed, `OCR failure`, `extra OCR language` | OCR language data failure: typed `ocr-language-data-failed`, import kept, manual review. Tesseract page recognize cannot be aborted mid-page |
| COMMIT_IMPORT | handle bookkeeping; commit closes the previous handle | `foliark-lifecycle.test.ts` (real wasm), `router.test.ts` import-rollback x4 | OCR-incomplete commit refused unless `allowIncompleteOcr` |
| CANCEL_IMPORT, CANCEL_REQUEST | `AbortSignal` per request id; `handle.close()` on discard | `cancel.ts` unit tests, lifecycle tests, e2e `cancel import` | cancel before commit leaves state unchanged; after commit the normal result stands |
| ACKNOWLEDGE_OCR_REVIEW | none | e2e `OCR failure ... the gate says OCR is incomplete` | |
| DETECT | none for rules; signature scan = TS port over `renderPage(i,1.5)` RGBA | e2e native detection (boxes within 0.004), `signature` | signature regions are an HDDN-side port (parity on 4 fixtures), not an engine feature |
| GET_PAGE_PREVIEW | `renderPage(i,{scale})`, PNG built in the worker | `foliark.test.ts` PNG sizes equal, e2e `repeated edit, undo` pixels | `CMAP_MISSING` render warning for legacy CJK CMaps (`cmapsUrl` unsupported in 0.1.0) |
| PREPARE_PARAGRAPH_EDIT | `prepareParagraph` | `foliark-edit.test.ts`, e2e edit cases (5) | horizontal, unrotated, left-to-right text only |
| LAYOUT_PARAGRAPH_EDIT | `layoutEdit` (Rust layout drives the red overflow outline) | e2e `overflow turns the outline red` | overflow refused, not baked; script or glyph it cannot set refused or falls back to the bundled font and says so |
| SET_TEXT_EDITS | `applyChanges` (`ChangeSet`, `TextEdit`), `RejectedChange` mapped by `noticeForRefusal` | `router.foliark.test.ts` rejected-edit, e2e `edit anyway`, `edit with keyword` | rejected edits come back per id with a reason; non-BMP and RTL replacement refused |
| APPLY_REDACTIONS native | `prepareExport`, `export` (sanitized rewrite), `ExportReport` shown as notices | e2e native, edit+redact; `check-exports.py`; acceptance 480 results (SAFE 388, REFUSED 58, EXPLAINED 34, 0 blockers) | refused with `native-*` codes (forms, optional content, invisible text, content under a pattern); no automatic retry |
| APPLY_REDACTIONS flattened | `export` with `policy.raster.acknowledgeLosses`, scale 2 | e2e `a refused native export asks before anything else`; `check-exports.py` raster; docker harness | only after the user reads the consequences; engine answers `CONSENT_REQUIRED` otherwise. Pages become pictures at display size |
| RESET | `handle.close()` for committed and pending, `resetDocumentState` | `router.test.ts` RESET, e2e `a lost worker is replaced` | engine `onReset` sets `documentFaulted`, HDDN-23 recovery replaces the worker |

No remaining Python path: `rg -i 'pyodide|pymupdf|fitz|mupdf'` over `src scripts public package.json astro.config.mjs` finds only the retired-engine oracle comments in three `foliark*.test.ts`, the service-worker eviction comment and its test, and the v16 note in `airplane-assets.ts` (NEW-71 receipt, re-run by its reviewer); `pnpm why pyodide pdf-lib` is empty; `dist` holds no pyodide, mupdf or wheel file; the built-network scan above saw none.

## Before and after
Source of the first table: `docs/performance/comparison.md` (NEW-62, worker-level medians, Chromium 151, same inputs for both engines; read its caveats before quoting a speedup).
| Metric | HDDN with PyMuPDF (2fe0f9a) | HDDN with Foliark | Note |
|---|---|---|---|
| engine download, cold | 30.41 MB | 5.41 MB | |
| init cold / warm | 1,596 / 1,416 ms | 18.8 / 12.4 ms | |
| native 30 pages: open + extract + render all | 1,001 ms | 223 ms | |
| image-heavy 6 pages: same | 11,565 ms | 662 ms | HDDN preview errors on scans before NEW-81, see comparison.md |
| paragraph edit apply | 1,004 ms | 69.1 ms | |
| native export, 10 regions | 399 ms | 249 ms | same 129 words removed, outputs differ in guarantees |
| raster export @2, 10 regions | 4,132 ms | 1,029 ms | different rasterizers |
| wasm memory after all ops | 249.69 MB | 74.84 MB | |
| `dist` size | 117,564 KB | 83,904 KB | NEW-71 receipt |
| `/redact` cold load | 26 requests, 7,125,373 B | 26 requests, 6,609,854 B (NEW-71) | |

Fresh on rc2, through the HDDN UI (`tools/hddn-acceptance/measure.mjs`, 5 runs, Chromium, localhost, no service worker, a deterministic 30-page native PDF from `tools/bench/make-native30.py`; UI-level, so not comparable with the worker-level rows above; the NEW-83 export sizes against PyMuPDF are in `docs/performance/comparison.md`):
| | HDDN 2fe0f9a (PyMuPDF), `hddn-2fe0f9a/dist` | HDDN pinned Foliark rc2 build |
|---|---|---|
| `/redact` cold load (median of runs) | 1,348 ms, 31 requests, 10,101,344 B | 617 ms, 26 requests, 6,482,580 B (byte counts vary between runs because bodies are counted as responses arrive; NEW-71 measured 6,609,854 B exactly) |
| native 30 pages: choose file to first text span | 2,473 ms | 1,280 ms |
| native export of the same file with the keyword `alpha` (Export click to file handed over) | 2,208 ms | 1,758 ms |
| exported file size | 28,926 B | **27,533 B** (rc1 build: 349,682 B; NEW-83 removed the overhead; Foliark is now below the PyMuPDF build for the same steps, and equals the controlled `export-size.py` result for this input: 27,500 vs 28,747 B) |
The old build's export and the rc2 export both came out of the same UI steps; the page and word counts of the old file were not checked, so no equivalence is claimed for it (the rc2 file is judged by the oracles above).

## Rollback
### Procedure (local branches only; nothing was deployed)
1. Rebuild the pre-Foliark app from the accepted ref in a scratch worktree: `git worktree add <dir> 2fe0f9a && cd <dir> && pnpm install --frozen-lockfile && pnpm run build` (through memory-queue). Proven here: `pnpm run build` in `~/ai/repos/foliark/.worktrees/hddn-2fe0f9a` exit 0, `dist/pyodide` present, service-worker cache name `document-redactor-airplane-v15`.
2. For production, ship that `dist` (or `git revert` the integration commits from `2c96b0b` back to `2cd43c5`, NEW-66 to NEW-71, on a release branch and rebuild). Do not edit the old checkout in place.
3. No cache bump is needed. The rolled-back service worker uses the older cache name (v15), so a browser holding the v16 cache installs v15, precaches the shell including Pyodide, carries the OCR data, fonts, detection lists, licences and locales across, and deletes v16 on activation. v16 never held Pyodide (the upgrade in NEW-70 dropped it), so users who had prepared offline tools before the rollback have the engine again only after the first online visit that installs v15.
4. Foliark engine files (`_astro/foliark_wasm_bg*.wasm`) are not served by the old build; v16 entries vanish with the cache.

### Test (`tools/hddn-acceptance/rollback.mjs`, exit 0, Chromium, one persistent profile, one origin)
- Re-run on rc2 (exit 0): same results as below (v16 127 entries to v15 134 entries, 14 Tesseract entries carried, 2 Pyodide entries, rolled-back build loaded Pyodide and 0 Foliark wasm, 1 page and 17 words before and after); offline after rollback still not established (60 s timeout).
- Phase 1, pinned build with a prepared-offline profile: caches = `[document-redactor-airplane-v16]` (127 entries), no Pyodide entry; open `multicolumn-unicode`, confirm all, export through the UI download path; saved file judged by PyMuPDF: email gone, 1 page, 17 words.
- Phase 2, same profile and origin now serving the 2fe0f9a build: after `registration.update()` the caches become `[document-redactor-airplane-v15]` (134 entries, 14 `tesseract/` entries carried over, 2 Pyodide entries precached); the page then loaded the Pyodide engine (2 Pyodide asm/wheel fetches, 0 Foliark wasm fetches), opened the same PDF, exported it through the UI, and the file is equivalent (email gone, 1 page, 17 words).
- Not established: offline re-preparation after rollback. The old build's offline dialog showed neither "Download tools" nor "Offline mode is ready" within 60 s in the run made, so no offline claim is made for the rolled-back build.
- Tested on a scratch worktree only; the main HDDN checkout was never used. The old build's own e2e is not part of this repository; its smoke here is the UI redact flow above. The 2fe0f9a docker harness was not re-run in this task.

## Remaining limitations
- Licence undecided (NEW-73); Foxit font terms beyond the PDFium notice unverified; no advisory check; no SBOM inside the tarball (`docs/release/candidate.md`).
- Mobile and real Safari not established (NEW-64, NEW-22). One machine; WebKit and Firefox are Playwright builds. The oracles judged the Chromium, Firefox and WebKit downloads (all PASS).
- Sanitize-only has no HDDN UI path. The oracle results are for the three exported fixtures and their declared secrets, not a privacy certificate; hidden-data coverage beyond that is the SDK acceptance suite's (`docs/evidence/security-acceptance.md`, `Not proven` list).
- Supported-vs-refused limits that remain on rc2: `cmapsUrl` unsupported (no CMap pack ships, legacy CJK CMaps render with `CMAP_MISSING`; owner decision 17.5); native export refuses (`native-redaction-unsupported`, `errors.nativeSharedContentPages`) while a marked graphic is drawn on an unmarked page (shared-image and shared-vector fixtures; marking that page or image-only exports; shared Forms with hidden text and shared page streams are unchanged); invisible text in HDDN native export uses `invisibleText: 'keepOcr'` (ADR 0008): kept only over a drawn picture (scan OCR), every other piece removed with notice `invisibleRemoved`; a hidden secret placed over a scan picture is indistinguishable from OCR and only a marking removes it; first render of a progressive RGB JPEG scan at scale 1.35 is about 0.9x the PyMuPDF-based HDDN (82.5 vs 76 ms, `docs/performance/comparison.md`), every other scan row is faster; the image-only raster check tolerates a 2 px rasterizer shift.
- Signature region detection is an HDDN-side TS port; `search`, `quadsFor`, `select` and `cmapsUrl` are not wired.
- WebKit blob console error from NEW-71: seen once (24/25 in the NEW-71 first WebKit run, in the extra-OCR case), **not reproduced**: 1 full WebKit run plus 3 isolated runs of that case here, all pass, plus NEW-71's own rerun (24/24). Cause not established. Likely area, unverified: a WebKit teardown of an in-flight `blob:` load (preview image or Tesseract worker) when the case navigates a second time. Nothing was changed in the app. The e2e now records the source URL of every console error, so a recurrence names the blob

## Coordination notes (evidence that now exists; no status was changed)
- HDDN-11 (sanitize hidden data before every native export): the engine-side evidence is `docs/evidence/security-acceptance.md` (480 results, independent oracles, negative controls 109/109, NEW-80 unused-resource pruning landed) and the UI exports above judged by `objects.py`. It replaces the PyMuPDF `pyodide.ts` sanitizer, which no longer exists on this branch. Its acceptance named PyMuPDF 1.26.3 behavior; that is now moot for the integration branch but the task text is unchanged.
- HDDN-18 (export-integrity regression gate): this branch has `validate` green (783 tests), the three-browser UI e2e (25/25 each), the independent oracle run on UI exports from all three browsers, and the isolated worker harness (Poppler, docker) 21/21 clean on rc2. HDDN-18 asks for strict pypdf and Poppler on the historical shared-image fixture; the isolated harness covers it (shared-image: native refuses by the shared-graphic rule, image-only and true-redaction checks clean), but the comparison against `f4f2aa59...` was not repeated here. No status was changed.
- HDDN-47 (cross-browser and constrained-device evidence): desktop Chromium, Firefox and WebKit engine evidence now exists for the Foliark path (25 cases each). Real Safari, iPhone, iPad and Android remain not established (NEW-64); HDDN-47 must not cite this document for them.
