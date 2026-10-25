# Changelog

All notable changes to `@zilijonas/foliark` (named `@foliark/sdk` before 0.1.0 was cut; that name was never published). Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: `docs/release/process.md` (semver; 0.x may break with an entry here).

## [Unreleased]

## [0.1.6] - 2026-10-25

Editing a one-line paragraph beside other text, and typed characters the page never drew. Source: tag `v0.1.6`. Tarball and hashes: `docs/release/candidate.md`.

### Changed
- A one-line paragraph with other text beside it (a table header with its neighbour column) now has `ParagraphInfo.wrapWidth` up to one space short of that text instead of its own width, so typing a longer header uses the free room before wrapping. A wrapped paragraph keeps the width it wraps at, and a line never reaches into the next column (ADR 0011 amendment) (NEW-119).

### Fixed
- The editor face claimed characters the page never drew with that font when the program's own cmap already agreed with the glyph evidence and also mapped more: a Word subset keeps every glyph slot with the outlines of unused glyphs emptied. The browser then drew such a typed letter blank (or as the wrong glyph) instead of falling back to the bundled font, while the layout set it in that font and the commit saved it. The cmap is now the evidence exactly, so the browser's per-glyph fallback matches the engine's ladder. Owner Word PDF (a bold subset with 4685 glyph slots): the editor drew `c f h q x G H J Q W X Y Z` blank; none now (NEW-120).

## [0.1.5] - 2026-10-25

Retyping over a selection gives the new text one look. Source: tag `v0.1.5`. Tarball and hashes: `docs/release/candidate.md`.

### Fixed
- Retyping over a selection in a paragraph with inline bold or italic text could split the typed words across looks: select all and type a sentence gave a regular first word and italic next words; select from a bold word to the end gave a bold word and an italic rest. The edit lined single spaces up as if they were shared words, so each typed word took the look of whichever old word its space landed on. Text typed over text now takes one look (the first replaced character's); words you kept keep theirs (ADR 0013) (NEW-115).
- A file with an empty name (a bare `/`) in a dictionary the export keeps, such as a font resource key, was written so the output read back as a different, incomplete dictionary and only reopened with repair. Export now refuses such a file with `MALFORMED_PDF` (found by fuzzing; 0.1.4 has the same defect) (NEW-117).

## [0.1.4] - 2026-10-24

Editing a paragraph keeps the bold, italic, size and colour of the words you did not touch, and apps can read a paragraph's styles before the user edits it. Source: tag `v0.1.4`. Tarball and hashes: `docs/release/candidate.md`.

### Added
- `ParagraphInfo.segments` (additive): the paragraph's style segments (ADR 0012) as UTF-16 ranges of `originalText` that rejoin to it exactly, each with its font (the page's own, never the caller's), size, colour, flags and editor face, so an app no longer calls `prepareParagraph` per row to learn the look of a mixed block. Empty when `originalText` is null (NEW-110).
- `ParagraphInfo.looks` and `ParagraphInfo.runs`, `EditPreview.styles` (additive, ADR 0013): the page styles a paragraph is drawn in and which UTF-16 range of `originalText`, and of the text a preview laid out, each one draws, so an editor can draw every run in its own face. Each look's editor face is transferred as a `Uint8Array` (NEW-113).

### Fixed
- Editing a paragraph with inline bold or italic text (a Word law article's bold defined terms inside regular body lines) turned those runs regular. Words you did not change now keep their own style (font, weight, italic, size, colour), and new words take the style of their neighbours. In similar-font mode, a bold or italic run uses the caller family's matching face, or the edit refuses `EDIT_NO_FONT` naming the characters when that face is not registered. Owner Word PDF: 14 of 14 inline bold terms of article 2 stay bold in the commit and the export after one body word is edited; owner iOS PDF still 0 untouched paragraphs refuse or re-break (ADR 0013). Fixture `generated/word-inline-runs.pdf` with a leaky and a safe oracle control (NEW-113).

## [0.1.3] - 2026-10-24

Edits on Word PDFs: untouched glyphs are no longer erased when fonts share a program, and editing a paragraph keeps its heading and body styles and stays inside the margin. Source: tag `v0.1.3`. Tarball and hashes: `docs/release/candidate.md`.

### Fixed
- An edit or a text redaction no longer erases glyphs of untouched text drawn by a font twin. Word embeds each font once and draws it through two font dictionaries, a WinAnsi TrueType and an Identity-H Type0, both pointing at one `FontFile2`; removing text emptied the outlines of every glyph the touched dictionary no longer drew, and the dictionary written last won. On the owner's Word PDF, editing one paragraph on page 1 erased the hyphen of an untouched heading (drawn by the TrueType alone) in the re-rendered page and the export, nine more hyphens on pages 10, 14 and 18 and 30 glyphs on page 20, while the text layer still read them. The program now keeps every glyph any font drawing from it still draws; ToUnicode stays per font. Fixture `generated/word-twins-shared-program.pdf`; the fidelity oracle gains GLYPHS (every untouched glyph drawn again with the same glyph id and the same ink), with two fail controls and one pass control (NEW-105).
- Editing the body of a block that holds a bold heading line over a regular paragraph (Word articles; HDDN joins them into one editable block) re-drew the whole block bold and up to 5.7 pt past the right margin while reporting fit: the paragraph took the first cluster's style, and the box was measured with that bold font. The selection now splits into style segments at hard breaks where the look changes; an edit removes and lays out only the segments it changes, each in its own style and box, and the rest stays on the page as it is. The paragraph style is the one most of its text is drawn in, and the box is measured in the page's fonts, never a wider caller font. Owner Word PDF: heading untouched, body regular, inside the margin; owner iOS PDF still 0 of 38 untouched paragraphs refuse or re-break (ADR 0012). Fixture `generated/word-bold-heading-body.pdf` with a leaky and a safe oracle control (NEW-106).

## [0.1.2] - 2026-10-24

Fixes for PDFs written by iOS (Quartz) and Word, found on real documents: no search results or selectable text, `<?>` in the editor, edits refused, and native redaction refused on pages with combining marks. Source: tag `v0.1.2`. Tarball and hashes: `docs/release/candidate.md`.

### Fixed
- Text of a font whose ToUnicode maps nothing (iOS/macOS Quartz writes Type0 Identity-H fonts whose ToUnicode declares only a codespace) is read from the embedded TrueType/OpenType program's own Unicode cmap, glyph id back to code point; it was all U+FFFD, so the page had no findings, a paragraph edit showed replacement characters and its commit was refused (`EDIT_UNSUPPORTED_SCRIPT`). The same fallback covers a code a partial ToUnicode or the encoding leaves unmapped; a symbol cmap is never read as text. Fixtures `generated/quartz-type0-empty-tounicode.pdf` and `generated/word-winansi-type0-twins.pdf` (Word's WinAnsi TrueType plus Identity-H twin, switching mid-word) (NEW-100).
- A zero-advance glyph (a combining mark: iOS Quartz writes "ą" as "a" plus U+0328, the mark with width 0) has no box of its own, and its zero-area quad made every region on that page refuse native export (`INVALID_ARGUMENT feature=textPositionUnknown`). A mark now takes the box and the line position of the glyph before it in the same show, so a region over the base removes the mark and a word with marks is one word, not split at each mark. Genuinely unplaceable text (singular CTM, size 0) still refuses. Fixture `generated/quartz-type0-combining-marks.pdf` with a leaky and a safe oracle control (NEW-102).
- A paragraph left as it is fits its edit box and keeps its line breaks. The box was the page's drawn extent while the layout measured the font's `/W` widths (or a fallback font's advances), and iOS Quartz draws 1/1000 em narrower than its `/W` through TJ corrections: on the owner's iOS PDF 10 of 38 untouched paragraphs reported "does not fit" and 11 re-broke; in the fixture corpus 18 of 72 one-word lines set in the Liberation fallback did. `wrapWidth` now holds the widest original line as the layout measures it and `maxLines` the lines the paragraph has now; longer text still refuses `EDIT_NO_FIT` (ADR 0011). A letter the source subset lacks precomposed (`ą`) is drawn as its base and combining mark when the subset has those, instead of `EDIT_NO_FONT` or a fallback font. Fixture `generated/quartz-type0-tight-widths.pdf` with a leaky and a safe oracle control (NEW-104).

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
