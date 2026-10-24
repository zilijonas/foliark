# ADR 0003: Paragraph metrics and edit application

Status: Accepted, 2026-10-22 (NEW-50, NEW-51). Refines spec §6.2, §6.3, §4. `wrapWidth` and `maxLines` amended by ADR 0011; the paragraph style and what an edit removes and draws by ADR 0012.

## Decisions
- `originX` is the minimum x over all selected clusters (spec text says the first line): a first-line indent survives an edit. HDDN uses the union x0.
- `originalText` joins soft wraps with one space and hard breaks with LF plus the indent as spaces (HDDN heuristic; the spec text says LF). A line break is hard when the line ends short of the paragraph's right edge or the next line starts indented.
- `wrapRight` follows the spec (max visible x on the page, or the paragraph's own max x when a word sits beside it). `bottomLimit` is the top of the nearest word below, from the spec's quads (ascent 0.8), without HDDN's -2 and not PyMuPDF's bbox.
- An edit is applied by removing the selected clusters through the redaction writer path (a must-remove set with one off-page region, so nothing else is taken out by overlap), then drawing the planned runs in a new `q..Q` content block. Source-font runs reuse the source font object by code and count as survivors, so removal keeps their outlines.
- The edit list replays from the source, one fresh writer per edit, and each edit's file is reopened strictly before the next edit's rects are read. A registered font is one object per (font id, document): its resource name and `BaseFont` tag derive from the font id, its Type0 dictionary records the glyph list (`/FoliarkGlyphs`, dropped by export), and the next edit embeds the union with the old glyphs keeping their CIDs and supersedes the old object.
- Working copies are written in preserve mode (§8.3, NEW-79): everything reachable from the trailer is kept with its keys verbatim, content streams are still re-emitted from tokens, and export reads the copy through the allowlist writer, so annotations, widgets, `/AcroForm` and `/OCProperties` are flattened or removed there as for any file. All failing edits are reported; an edit after a failed one is judged on the state without it.
- A new working copy renumbers objects: every stored text redaction is re-resolved by text and geometry (0.05 pt); one that cannot be found refuses the ChangeSet (`redactionTargetEdited`), so privacy never silently drops.
