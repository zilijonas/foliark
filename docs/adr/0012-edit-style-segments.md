# ADR 0012: An edit keeps each style segment's own style

Status: Accepted, 2026-10-24 (NEW-106). Amends spec §6.2 (paragraph style) and §6.3 (what an edit removes and draws); amends ADR 0011 (the font the box is measured with). ADR 0004 is unchanged: preview and commit still call one `plan_edit`.

## Context
Owner's Word PDF (Arial and Arial-Bold as TrueType WinAnsi subsets plus Type0 Identity-H twins), page 1, in Safari on 0.1.2: HDDN's editable block for article 1 is the bold heading line plus the regular body below it, because HDDN joins lines by geometry only (`groupSpansIntoParagraphs`). Editing one body word re-drew the whole block bold, 5.7 pt (about 9 px) past the right margin, while the layout reported fit. Engine-level causes (counts and geometry only):
- §6.2 took the style of the first selected cluster: the heading's. Rung 2 was the bold subset, so every line was drawn bold (`paragraph.rs` `prepare`, `style = styles[clusters[first]]`). HDDN's fallback pick (asked because the block is `mixedStyle`) follows `info.bold` too, so its caller font was bold as well.
- ADR 0011 widens `wrapWidth` to the original lines as the layout measures them, with the edit's ladder: the regular body rows measured in bold came out 487.1 pt against a page extent of 482.0 pt, so bold text "fit" a box 5.1 pt wider than the page (`paragraph.rs` `measured`).
- The edit removed and redrew the whole selection, heading included, at one `lineStep` from the first baseline.
On the whole file 66 of 149 such blocks hold more than one style segment: 18 boundaries are a bold heading over its body, 55 an italic amendment note under an article.

## Options
- (a) Split blocks at style boundaries in extraction (§3.3). Breaks PyMuPDF block parity (§3.6), and HDDN does not use extraction blocks for its edit unit.
- (b) Keep per-run styles inside one layout. Needs a character diff to give inserted text a style, and one `lineStep` still moves the body under a heading with paragraph spacing.
- (c) Split the selection into style segments and lay out only what changed, each part in its own style and its own box.

## Decision
(c):
- Paragraph style (§6.2) is the style most selected clusters are drawn in (ties: the earliest), not the first cluster's. `mixedStyle` is unchanged.
- Style segments: the selection's rows in order; a new segment starts at a hard break (`originalText` LF) where the row's look changes. A row's look is the style most of its clusters have, compared by font name (subset tag stripped, so Word's twins agree), size, color and flags. A soft break never splits.
- An edit lines its text up with `originalText` by LF-separated lines: a segment boundary keeps its place when the line count is unchanged, when the line before it is in the common prefix, or when the line after it is in the common suffix; otherwise the segments on both sides go together. The changed part runs from the first segment whose lines differ to the last. Only its clusters are removed and only it is laid out, prepared on its own (its style, rung 2 font, `firstBaseline`, `lineStep`, `wrapWidth`, `maxLines`), so the segments around it stay on the page as they are and bound its budget like any other text. Text equal to `originalText` changes nothing. One segment, or `originalText` null, is the old behaviour.
- `wrapWidth`'s ADR 0011 measurement uses the fonts the page draws the text with: the ladder without the caller's font (`TextEdit.font`). A caller font wider than the page's re-breaks or refuses `EDIT_NO_FIT`; it never moves the right edge.
- Preview glyph offsets (ADR 0004) still index the whole normalized text the caller sent; `lines`, `neededLines`, `maxLines` and the fingerprint are the changed part's.

## Consequences
- Owner Word file, article 1, one body word changed, with no caller font, with a regular and with a bold caller font: the heading is the page's own text, where it was (29 glyphs, bold); the body is regular except where the caller asked for bold; the widest line ends 34.1 pt (20.3 pt with the bold caller) inside the original right edge, against 5.7 pt past it before. Unchanged text: 0 of 149 blocks refuse or overflow with registered fonts; 66 multi-segment blocks are no-ops; re-broken blocks 27 to 14. Owner iOS file: 0 of 38 refuse or re-break, as before.
- Limit: changing text in two style segments at once (a heading word and a body word) lays the segments between them out together in their main style, as one segment was before. Separate edits keep both.
- HDDN's overlay still draws the block with one face (`editorFace` is the paragraph style's); the commit is right, the in-editor look of the other segment is not. HDDN can draw each preview glyph with its run's font.
- A segment that ends short of the right edge and is followed by a row in another look is a hard break by §6.2's rule, so Word headings, notes and bodies split as expected; a style change mid-line or at a soft wrap does not split and is flattened to the main style, as before.
- Addendum (NEW-110, additive): `prepareParagraph` returns the segments as `ParagraphInfo.segments` so an app does not re-derive them. Each is a UTF-16 range of `originalText` (the ranges tile it; a segment's closing LF belongs to it) with its look and the page's own font (never the caller's), as the segmentation above defines them. Empty when `originalText` is null.
