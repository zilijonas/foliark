# HDDN compatibility report (NEW-89)

What a valid PDF can still hit in HDDN on Foliark, kept apart from what is refused on purpose because the file is broken, hostile or past an advertised limit. Written from the fixture corpus and the recorded acceptance runs, not from field data: **no real-user failure rate exists and none is claimed.** The corpus is synthetic or authored by the project (`tests/fixtures/manifest.json`: 40 files, 20 from HDDN's own fixtures, 20 generated); how often real documents look like these cases is unknown.

Sources: `docs/evidence/security-acceptance.md` (80 cases x 6 scenarios = 480 results: SAFE 388, REFUSED 58, EXPLAINED 34, 0 blockers), `docs/evidence/strict-native.md` (39 cases = 31 supported + 8 refused), `docs/integration/hddn-acceptance.md`, HDDN `tests/e2e/foliark-viewer.mjs`.

## 1. Refused on purpose (broken, hostile or past a limit; not compatibility gaps)
| Input | Typed outcome | Why it stays refused |
|---|---|---|
| `malformed-truncated`, `malformed-xref` | `REPAIRED_PDF` | HDDN opens with `allowRepaired:false`: a repaired xref could hide content the reader never showed |
| `corrupt-flate` | `DECODE_FAILED` | A stream that does not decode cannot be proved clean; no empty-on-failure fallback |
| `forms-cyclic` | `MALFORMED_PDF` | A Form that draws itself |
| `forms-deep-40` | `LIMIT_EXCEEDED` (`maxNesting`) | Nesting depth is a resource limit, not a feature gap |
| `oversized-image` (10^10 pixels), `compressed-stream` (8 MiB no-op stream) | `LIMIT_EXCEEDED` | Decompression and dimension bombs (HDDN's fixtures require the refusal) |
| `many-pages-1001` | `LIMIT_EXCEEDED` (pages) | A valid file, refused by HDDN's 30-page limit and the engine's limits. The UI now says to split it |
| A file over 25 MB | refused before the engine | Same, "Compress it or split it" |

## 2. Valid files that hit a boundary (gaps, each with the user's way out)
| Case | What happens | Early warning | Way out | Evidence |
|---|---|---|---|---|
| Marking over a tiling-pattern fill that holds text or pictures (`pattern-text-cell`, `pattern-image-cell`, HDDN `pattern-secret`) | native `REDACT_PATTERN_UNSUPPORTED` with pages | yes, pages named | consented image-only PDF, which burns the marked area | acceptance R; HDDN e2e `early warning` |
| Marking over content under a soft-mask group (`smask-group-text`) | native `INVALID_ARGUMENT redactionSoftMaskGroup` | yes | same | acceptance R |
| A marked picture or Form is also drawn on an unmarked page (`shared-image`, `shared-vector`) | HDDN adapter stops native export (the engine keeps the original for the other page) | yes, unmarked pages named | mark it on those pages, or image-only | `foliark-export.test.ts` (real wasm) |
| Form widget without an appearance stream (`FORM_APPEARANCE_MISSING`), XFA | native refused | yes (capabilities preflight) | image-only, if every page draws | spec 11 |
| Image-only PDF when a page draws with a warning: non-embedded non-standard font (`FONT_SUBSTITUTED`), unreadable image (`IMAGE_DECODE_FAILED`), legacy CMap not shipped (`CMAP_MISSING`) | `RENDER_INCOMPLETE` with pages. Kept a refusal: the picture would silently lack content | yes: "the image-only PDF would stop too: pages ..." | none in HDDN for those pages (use the native route, which is unaffected) | `raster_plan.rs`; stub-engine test |
| Image-only PDF when a comment has no appearance stream (`ANNOTATION_APPEARANCE_MISSING`) | HDDN retries with the comments removed and discloses the count | yes, pages named | none needed | `annotation-comment` unit test |
| Non-embedded Chinese, Japanese or Korean fonts | text reads through the shipped predefined CMaps (NEW-88, copied to `foliark-cmaps/` at build). The glyphs are drawn with a Latin stand-in (`FONT_SUBSTITUTED`), so image-only refuses those pages. Character data that cannot be fetched stops the import with a message | no (the document opens and exports natively) | retry the import online | HDDN e2e `character map that cannot be fetched` |
| Text edits on rotated text, mixed-length encodings, non-BMP or RTL replacement | edit refused per paragraph | n/a (export unaffected) | edit elsewhere | spec 6 |

Known and not distinguished: `FONT_SUBSTITUTED` is treated like any other render warning by the image-only route. A substitution with identical metrics (for example a non-embedded Arial drawn with a metric-compatible face) is harmless to the picture, but the engine does not yet say so, and HDDN does not override `RENDER_INCOMPLETE`. A change would belong in the engine (the warning would carry a verified-metrics flag) with a fixture and a pixel oracle. Not done in NEW-89.

## 3. How the early warning works (HDDN, NEW-89)
After import and after each change to markings or committed edits (debounced, never while a paragraph is open or an export is running) HDDN asks the worker `CHECK_EXPORT`:
1. `applyChanges` with the markings, then `capabilities()` (cheap: walks the catalog and the redaction plan, nothing interpreted twice). `exportNative: unsupported` carries the engine's own code and pages.
2. Otherwise, when something would be exported, one real native `prepareExport` (the only proof of support, spec 11). Its held bytes are freed at once by the closing `applyChanges` with no redactions. A refusal or `sharedSurvivors` becomes the warning.
3. Only when native is refused, each page is drawn at scale 0.1 and its warnings read (a page with a warning is a page the image-only export would refuse). Result: image-only available, blocked pages, or comment-only pages whose notes would be dropped.
The warning is a status line, not a dialog; nothing is started. It steps aside once an export has actually refused (the refusal and the consent prompt carry the same facts). It is silent on failure: the export itself still decides.

Not detected early: output-size, work-budget and memory limits (they depend on the final bytes, so they are reported when they happen, with what to change), and an image-only failure that only appears at scale 2 (for example a picture budget).

## 4. Work is kept on every failure path
Edits, markings and review state live in the app store and the engine's acknowledged edit list; export never resets them. Covered by browser tests on the built app: refusal with edits and a marking (they stay, consent then exports them burned in), cancel after generation (bytes dropped, nothing saved, retry saves the same edit and keyword), save-picker cancel and retry, write failure and no-picker download (earlier cases). Nothing is persisted and "saved" is claimed only after a complete write.
