# Changelog

All notable changes to `@zilijonas/foliark` (named `@foliark/sdk` before 0.1.0 was cut; that name was never published). Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: `docs/release/process.md` (semver; 0.x may break with an entry here).

## [Unreleased]

### Fixed
- Text of a font whose ToUnicode maps nothing (iOS/macOS Quartz writes Type0 Identity-H fonts whose ToUnicode declares only a codespace) is read from the embedded TrueType/OpenType program's own Unicode cmap, glyph id back to code point; it was all U+FFFD, so the page had no findings, a paragraph edit showed replacement characters and its commit was refused (`EDIT_UNSUPPORTED_SCRIPT`). The same fallback covers a code a partial ToUnicode or the encoding leaves unmapped; a symbol cmap is never read as text. Fixtures `generated/quartz-type0-empty-tounicode.pdf` and `generated/word-winansi-type0-twins.pdf` (Word's WinAnsi TrueType plus Identity-H twin, switching mid-word) (NEW-100).
- A zero-advance glyph (a combining mark: iOS Quartz writes "ą" as "a" plus U+0328, the mark with width 0) has no box of its own, and its zero-area quad made every region on that page refuse native export (`INVALID_ARGUMENT feature=textPositionUnknown`). A mark now takes the box and the line position of the glyph before it in the same show, so a region over the base removes the mark and a word with marks is one word, not split at each mark. Genuinely unplaceable text (singular CTM, size 0) still refuses. Fixture `generated/quartz-type0-combining-marks.pdf` with a leaky and a safe oracle control (NEW-102).

## [0.1.1] - 2026-10-22

Engine fixes from HDDN's public-corpus gate (NEW-94) and the npm page polish. Source: tag `v0.1.1`. Tarball and hashes: `docs/release/candidate.md`.

### Fixed
- A legitimate large scan imports: an image XObject is held to its declared size (width x height x components x bits, one predictor byte per row, never below `maxStreamDecodedBytes`) instead of the generic 128 MiB stream cap, so a 9000 x 9000 RGB scan (81 MP, 243 MB decoded) opens, renders and exports. `maxImagePixels` still refuses dimension bombs, the cap still stops a stream that inflates past its declaration, and the redaction image budget no longer double-counts against the stream cap (ADR 0010).
- Annotations with the `NoRotate` flag (Text notes) are flattened upright with the upper-left corner of their rectangle fixed on a rotated page, as viewers draw them; they used to turn with the page and move (HDDN `public-cropped-rotated`, 0.67 to 0.78 percent of pixels changed on three pages, now 0.14 to 0.18) (ADR 0010).
- Opening a file no longer holds a Flate-only image in memory just to measure it: the budget scan counts the inflated bytes in a fixed buffer (the 81 MP scan opened at +524 MB of memory, now +17 MB).
- The first draw of that scan peaks at +230 MB instead of +510 MB: the Flate decoder allocates an image's expected size once instead of doubling (hayro P21).
- Native export no longer refuses with a shared-content error when two pages carry byte-equal annotation appearances and one page is redacted: the staged copy keeps flattened appearances apart (they were merged by object deduplication).
- The raster renderer places annotation appearances with the origin scaled with the box and honours `NoRotate` the same way (hayro patch P20); image-only export of rotated pages showed the same note displacement.
- Regression tests: edge-straddling text, fill and image keep their pixels on every page rotation while wholly outside content goes (`sanitize_visibility.rs`); NoRotate notes against MuPDF at 90, 180 and 270 degrees (`sanitize_interactive.rs`); the 81 MP scan and the lying-stream bomb (`resource_limits.rs`).

### Changed
- npm package page: new `README.md` with the logo, the HDDN story, the measured comparison with its context and caveats, a feature list, quick start, browser support and licensing, and a not-affiliated note for PyMuPDF and MuPDF.
- `package.json`: new `description` and `keywords`; version 0.1.1.
- Repository `README.md` carries the same story and pitch; the stale wasm size in the package README is corrected to 5.8 MB raw, 2.2 MB gzip.

## [0.1.0] - 2026-10-22

First public release, `@zilijonas/foliark`, AGPL-3.0-only with separate commercial licensing (`LICENSING.md`, `COMMERCIAL-LICENSING.md`). Source: tag `v0.1.0` of https://github.com/zilijonas/foliark. Candidate tarball, hashes and checks: `docs/release/candidate.md`.

### Added
- `createInlineFoliark({ fetchCMap })` reads CMap pack files where `fetch` cannot (the Node `file:` default); NEW-88 follow-up.
- Headless Rust/WASM PDF SDK that runs in a worker, with no network after the files are cached: open (RC4-40/128 and AES-128/256 encrypted input with the password), extract text, search, hit test, select, render.
- Paragraph edit for horizontal, unrotated, left-to-right text, with stale-revision detection.
- Native export: edit, flatten, redact, sanitize, rewrite, verify. Independent oracles found no leftover content and no collateral damage on the cited corpus (`docs/evidence/strict-native.md`).
- Raster export (image-only, redactions burned in), behind explicit `acknowledgeLosses` consent.
- Typed refusals and warnings instead of silent fallback (`MALFORMED_PDF`, `ENCRYPTION_UNSUPPORTED`, `SIGNED_INPUT`, `LIMIT_EXCEEDED`, `RENDER_INCOMPLETE`, `CMAP_MISSING` and others), cancellation, worker watchdog and recovery.
- Scanned pages render fast: scaled image decode and a per-document reduced-image cache, so a re-render after an edit or a preview refresh no longer decodes the scan again (NEW-81).
- Notices (`THIRD_PARTY_NOTICES.md`, `NOTICE`, `LICENSE_FOXIT`), support matrix in the package, CycloneDX SBOM next to the tarball, release workflow and local dry run.

### Changed
- Redaction cuts filled vector paths and clip outlines precisely instead of removing a whole path for crossing a region: region subtraction in display space (`i_overlay`, nonzero and even-odd, holes, curves, any transform, nested clips). Cut curves become 0.01 pt polylines (loss `VECTOR_CUT_APPROXIMATED`); `RedactionReport.vectorRectsCut` is now `vectorPathsCut`; fills no longer produce `VECTOR_COLLATERAL`. Over 250,000 vertices or 131,072 pt coordinates is `LIMIT_EXCEEDED` (`vectorVertices`, `vectorCoordinate`) (NEW-86, ADR 0009).
- Redaction cuts stroked paths precisely too: the stroke is expanded in its own space (width, caps, joins, miter limit, dashes, non-uniform CTM), the regions are subtracted and the rest is written as a filled outline in the stroking colour; strokes clear of every region stay byte-exact, and `VECTOR_COLLATERAL` is never produced (`RedactionReport.collateral` stays, always empty). A cut stroke whose paint a fill cannot reproduce is refused with the new `REDACT_VECTOR_UNSUPPORTED {feature, pages}` (`strokeColorInherited`, `strokePattern`, `strokeAlphaDiffers`, `strokeBothAlpha`); over 100,000 dashes is `LIMIT_EXCEEDED {vectorDashes}` (NEW-87, ADR 0009, hayro patch P19).
- Native export is smaller: one content stream per page, identical objects stored once, compact operands. On the keyword flow of `tools/bench/export-size.py` every exportable fixture is at or below PyMuPDF's size for the same input (native-30p 27,500 B vs 28,747 B), checked against `tools/bench/budgets.json` (NEW-83).

### Fixed
- The decode-ratio limit no longer refuses flat image XObjects (a blank 150 dpi scan was refused at 1026:1); stream bombs and the 10^10-pixel image still refuse, with regressions in `resource_limits.rs` (NEW-82).
- HDDN's isolated export-integrity findings were triaged: the engine fix is the limit above; the rest were adapter policy or check tolerance (`docs/integration/hddn-acceptance.md`). Native export still refuses (`native-redaction-unsupported`) while a marked shared graphic is drawn on an unmarked page.

### Known limits
- Browsers: Chromium (desktop, macOS arm64) supported with budgets; WebKit engine supported, not as Safari; Firefox functional only. Real Safari, iPhone, iPad, Android, Windows and Linux: not established (NEW-64, device access NEW-22). No minimum browser version is promised.
- Predefined CMaps (legacy CJK encodings) ship as a self-hosted pack in the package (`cmaps/`, 63 Adobe files, BSD-3-Clause), fetched at `open` from `cmapsUrl` (default: the package folder) and cached by the caller for offline use.
- Paragraph edit does not cover CFF/Type1 embedded faces, RTL, CJK, vertical text or auto-fit.
- Encrypted raster fidelity, abort while an operation runs through the packed SDK, and files unlike the synthetic corpus are not proven.
- Budgets were frozen on one machine (Apple M4); reproducibility across machines is not shown.
- Foxit font terms beyond the PDFium notice are unverified; cargo-deny runs without an advisory check; no legal clearance of the notices.
- See `docs/support-matrix.md` for every row and its evidence.

[Unreleased]: docs/release/process.md
[0.1.1]: docs/release/candidate.md
[0.1.0]: docs/release/candidate.md
