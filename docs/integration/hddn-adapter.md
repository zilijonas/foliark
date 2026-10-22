# HDDN adapter (NEW-66)

HDDN worktree `~/ai/repos/hddn-foliark`, branch `foliark/integration` (from `codex/hddn-export-buffer` 2fe0f9a), local only. The adapter is
`src/workers/lib/foliark.ts` there; this page is the contract it implements and what is left.

## Status (HDDN worktree `~/ai/repos/hddn-foliark`, branch `foliark/integration`, local commits, nothing pushed)
| Task | What | Commit | State |
|---|---|---|---|
| NEW-66 | adapter: load, inspect, extract, render, lane, failure codes | 2cd43c5 | done |
| NEW-67 | signature scan, OCR route, selection/search proof, real-browser e2e | 2ac3d92 | done |
| NEW-68 | paragraph editing on `prepareParagraph` / `layoutEdit` / `applyChanges` | 61fc5bd | done |
| NEW-69 | export: native redaction, image-only fallback behind explicit consent, report notices | 7a19c1b | done |
| NEW-70 | AbortSignal cancellation, typed OCR failure, cache name bump, 3-browser e2e | 1c7d255 | committed, owned by another agent (board is the source of truth) |
| NEW-71 | remove Pyodide and the `PUBLIC_PDF_ENGINE` switch | - | not started |
| NEW-76 | complete-replacement and rollback proof | - | blocked on NEW-71 and this candidate |

SDK surface HDDN relies on: `createInlineFoliark`, `open`, `pages`, `extractText`, `renderPage`, `prepareParagraph`, `applyChanges`, `prepareExport`, `export`,
`FoliarkError` codes, and the exported types `ChangeSet`, `TextEdit`, `Redaction`, `ExportReport`, `Loss` (NEW-74 added them to the public index so the adapter no longer derives them from `ChangeSet`).
Not wired: `search`, `quadsFor`, `select` (nothing in HDDN's UI would use them), `cmapsUrl` (no pack ships).

## Decision: in-worker entry, not a nested worker
HDDN already runs one dedicated worker (`redactor.worker.ts`) whose router serializes whole requests and whose `documentFaulted` / generation
recovery (HDDN-23) replaces the worker on a fatal error. A second Foliark worker underneath would give two supervisors, two generations, a
`postMessage` hop per call and a recovery story that must keep both in step. So the SDK gained `@foliark/sdk/inline`
(`createInlineFoliark`, `packages/sdk/src/inline.ts`, ~25 lines): the same `Foliark` client and worker core over a `MessageChannel` inside the
calling thread. Same DTOs, same `FoliarkError` codes, buffers still transfer, no `worker.js` asset. Consequences, documented in `docs/deployment.md`:
the hang watchdog is off (the host supervises); after any `onReset` the host replaces its whole worker (the adapter sets `documentFaulted`, which
feeds HDDN's existing recovery path); `wasm` option (bytes or module) lets tests and prefetched caches skip the fetch.

## What the adapter maps
| HDDN | Foliark | Notes |
|---|---|---|
| INIT / PRELOAD | `createInlineFoliark({wasmUrl, limits:{maxInputBytes:25 MiB, maxPages:30, maxOpenDocuments:2}})` | wasm URL from `@foliark/sdk/wasm?url` (hashed, same file in the page build and the worker build) |
| LOAD_PDF | `open(bytes,{password,allowRepaired:false})`, `pages()`, `extractText(i)` per page in order | HDDN's `%PDF-` / `%%EOF` precheck kept; password cleared by the router as before |
| failure codes | `MALFORMED_PDF`, `REPAIRED_PDF`, `DECODE_FAILED`, `ENCRYPTION_UNSUPPORTED` -> `malformed-pdf`; `PASSWORD_REQUIRED/INCORRECT`; `LIMIT_EXCEEDED{limit:~page}` -> `too-many-pages`; other `LIMIT_EXCEEDED`, `OUT_OF_MEMORY` -> `resource-exhausted` | thrown as `LocalizedError` with the keys the Pyodide path used |
| stage / activate / suspend / commit / discard | handle bookkeeping: one committed handle, one pending handle per import id, an `active` pointer previews render from | same transition names `document-import.ts` already used; commit closes the old handle |
| `PageAsset` | `PageGeometry` width/height/rotation (display space) | |
| `TextSpan` | `Word`: box normalized with spec 2 (`clamp01`), `span_{page}_{i}`, UTF-16 `start/end` from `TextPage.text`, block/line/word keys, `source:'native'`, `confidence:1` | no copy of Foliark internals; SDK types stop at `foliark.ts` |
| lane | HDDN rule unchanged on `words.length`, code-point length of `text`, `stats.imageCoverage`, U+FFFD count and the garbled ratios | `unmappedGlyphs` is already the U+FFFD count in `text`, so it is not read separately |
| GET_PAGE_PREVIEW | `renderPage(i,{scale})` -> RGB PNG made in the worker (CompressionStream, no canvas) | the UI still receives `image/png` bytes; unrotated crop for OCR by undoing `/Rotate` on the RGBA |

## Switch
`PUBLIC_PDF_ENGINE=pyodide` (build) or `INIT {engine}` (runtime) keeps the PyMuPDF path; default on this branch is Foliark. Under Foliark every operation runs on the engine
(NEW-66 to NEW-69); an operation the engine refuses fails with its mapped HDDN code and is never retried on Pyodide (no silent fallback). Pyodide is not booted under Foliark.
The switch goes away with NEW-71.

## Parity (tests: `src/workers/lib/foliark.test.ts`, `router.foliark.test.ts`)
Oracle: HDDN's shipped `stage_document_from_bytes` under native PyMuPDF 1.26.5 (app ships 1.26.3), recorded by
`scripts/pdf-fixtures/record-pymupdf-expectations.py` into `tests/foliark/pymupdf-expected.json`. 21 fixtures plus encrypted no-password and wrong-password.
Equal: page count, size, rotation, lane, `textContent`, `charCount`, every span's text, offsets, block/line/word keys, failure codes, preview and OCR PNG sizes.
Boxes: worst edge deviation 0.211 pt (spec 3.6 allows 0.75).

Accepted differences:
- `actualtext`: MuPDF returns the marked-content ActualText (`SECRET_ACTUALTEXT`) for a word whose glyphs show `PUBLIC_GLYPHS`; Foliark extracts what is drawn. The hidden string is
  not searchable or detectable on screen; removing it at export is the sanitizer's job (spec 8, NEW-69).
- `oversized-image` (100000 x 100000 px, 10^10 pixels from 3 payload bytes): Foliark refuses at `maxImagePixels` while PyMuPDF loads it. The fixture's `inputPolicy` asks for explicit resource rejection, so `resource-exhausted` is correct.
- `compressed-stream` (8 MiB of no-op operators, decode ratio 1001): refused at `maxDecodeRatio`; it is a content-stream bomb and the fixture's `inputPolicy` asks for bounded decode or explicit rejection. NEW-82 exempted image XObjects from the ratio (a blank 150 dpi RGB letter scan, 6.3 MB of 0xFF, ratio 1026, was refused too; `maxImagePixels` and `maxStreamDecodedBytes` still bound images).
- Page sizes ignore `/UserUnit` in PyMuPDF and include it in Foliark (normalized boxes are invariant); no fixture sets it.

## Limits of NEW-66 (historical: NEW-67 added scanned fixtures and the signature scan, NEW-70 bumped the cache name)
Scanned-page OCR is wired through the same `renderPng(...,true)` seam but has no scanned fixture here; signature regions return none under Foliark;
the offline precache lists the wasm but the service-worker cache name was not bumped (NEW-70).

## NEW-67: signatures, OCR, selection, search, browser proof (HDDN `foliark/integration`)
- Signature scan: `detect_signature_regions` ported to TS (`src/workers/lib/signature-regions.ts`) over `renderRgba(page, 1.5)` from the active Foliark handle (gray like MuPDF `csGRAY`, same 540 px grid, thresholds and text-overlap veto). Parity vs PyMuPDF on 4 fixtures at both vertical gates: same region counts, boxes within 0.02; ranking by ink area can swap near-ties by 1 px between rasterizers, so tests compare by position.
- OCR: already routed through `renderPng(page, ocrScale, true)` (NEW-66); now proven in a real browser on a scanned and a mixed fixture. OCR spans merge unchanged (`source:'ocr'`).
- Selection: HDDN selection is DOM word spans placed from span boxes (no engine call), so equivalence is the word-box parity (0.211 pt worst). Search: HDDN has no document search (the watchlist is JS regex over `textContent` + span offsets). SDK `search`/`quadsFor`/`select` are verified on the multi-column Unicode fixture (diacritic folding with `diacriticSensitive:false`, UTF-16 range to word quad, column drag stays on its line) but NOT wired into the UI: nothing there would improve.
- Real-browser proof: `pnpm run test:e2e:foliark` (6 cases, chromium, built `/redact`), see `tests/pdf/README.md` in HDDN.
- Findings: Tesseract start-up failure (unreadable or unreachable language data) makes HDDN's `errorHandler` throw in the worker: UI shows the worker-crash/recovery banner and the import is lost (engine independent, tesseract.ts). Page-level OCR failure behaves as designed (warning, manual-review gate).

## Next
NEW-71 (remove Pyodide once 66 to 70 are accepted), then NEW-76. Open items carried here: scanned-page OCR timing is not part of the engine comparison, signature regions are an HDDN-side port over `renderRgba`.
