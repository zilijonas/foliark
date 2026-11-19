# ADR 0019: aligned paragraphs and the width tolerance (NEW-139)

Status: Accepted, 2026-11-19. Amends spec 6.2 (paragraph metrics), 6.3 (layout) and ADR 0015 (layout order).

## Context
HDDN (0.3.2) edits the right-aligned date block of an invoice ("Sąskaitos data: 2026-10-12" over "Apmokėti iki: 2026-11-12",
both rows ending at one x). Replacing a date with one a hair wider wrapped its last digits to a third line, and the preview said
`fits: true`. Each further edit saw that extra line, a seeded hard break and a stray indent, so the block degraded. Three causes:
- The engine had no notion of alignment. A right-aligned block was laid out left-aligned from its left edge; HDDN imitated the
  shorter row's offset with leading spaces, which the engine counted as an indent. Any edit that changed a row's width left the
  right edge ragged.
- The block's widest row already ends at the paragraph's right edge, so the stable width (spec 6.2) has no slack.
- ADR 0015 tries the extended width only when the text does not fit the line budget. The budget includes the room below, so a
  third line "fits" and the extended width is never tried. Ordinary left-aligned paragraphs have the same failure when an edit is
  a hair wider than the stable width.

## Decision
1. **Alignment from geometry** (`ParagraphInfo.align`, optional; absent from engines before 0.3.3 and read as `left`). Rows are the
   selection's lines after overprint de-dup (spec 6.2). With `tol = 0.1 em` (paragraph font size) and `ragged = space / 2`:
   - two or more rows are `right` when their right edges agree within `tol` and their left edges differ by `ragged` or more;
     `center` when their centres agree within `tol` and both edges differ by `ragged` or more;
   - a lone row is `right` only when another line nearby (centre within 10 em, not on its row) ends within 0.05 em of where it
     ends and starts at least `ragged` away from where it starts, and the row spans at most half the page's text width (a full or
     justified line ends at the right margin whatever its alignment); that line is the last word of its row plus the words before it up
     to a gap of two spaces. Neither may start within a space of the page's left text margin (a full-width line ends at the right
     margin whatever its alignment). A lone row is never `center`;
   - a row stretched past its text (`width - glyphs > 1.5 space` per word gap: justified word spacing) makes the selection `left`;
   - anything else, an unmapped selection included, is `left`: the layout before this ADR.

   Tolerance: an aligning producer puts every row's end at one pen position, so ends agree to its rounding and to the difference
   between drawn advances and the program's (`/W` against `hmtx`, TJ corrections), a few thousandths of an em per glyph. The
   owner's pdf-lib invoice agrees to 0.01 pt; the same block after a 0.3.1 edit (rows re-laid from the left edge in the layout's
   own advances) is 0.76 pt (0.084 em) apart, which `tol = 0.1 em` still reads as right-aligned and repairs. Rows with different
   text differ by a glyph at least; the narrowest glyphs are about a space wide (0.25 em), so `ragged = space / 2` stays above
   `tol`. A lone row's evidence comes from elsewhere on the page and is held to the tighter 0.05 em. Justified text reads as
   `left` (it is laid out as before).
2. **Aligned text.** Every break of an aligned block is hard: its rows are lines set one by one, so the text holds LF between
   rows and never leading spaces. In an aligned layout a line's leading spaces are ignored (old clients seed them), so they change
   nothing.
3. **The box.** `right`: the right edge `R` is the rows' right edge and `originX + wrapWidth = R` for every width; the box grows
   leftward. `center`: the centre `C` of the selection stays at `originX + wrapWidth / 2`. The room on the left mirrors spec 6.2's
   right: the stable left edge is the selection's left edge when another word lies left of it on its lines (a lone row: up to that
   word's right edge plus a space), else the page's left text margin; the extended left edge is the nearest obstacle on the left
   plus a space, a container's left edge plus a space, or `min(left margin, W - max x of visible text)`, never right of the
   selection. `right`: `wrapWidth = max(R - stableLeft, measured)`, `maxWrapWidth = max(wrapWidth, R - extendedLeft)`.
   `center`: twice the nearer side's room. `maxLines`/`maxWrapLines` and push-down's column (ADR 0015) read the box's own column
   `[R - w, R]` (`[C - w/2, C + w/2]`), at least as wide as the selection. Each line `x = R - width` (`C - width / 2`).
4. **Width tolerance** (ADR 0015 amendment). After the stable layout fits but needs more lines than the paragraph has now (or the
   hard lines typed, if more), the engine tries a wider box: up to 1 em more for `left`, up to `maxWrapWidth` for an aligned box,
   never past `maxWrapWidth`. When that keeps the lines, the narrowest such width (bisection to 1/4096 of the extra) is used, at
   the `maxWrapLines` budget, so lines the user did not touch take no words from below that they did not need. Otherwise the stable
   layout stands. Text that keeps its lines at the stable width, or needs a new line even 1 em wider, is laid out as before.
5. Preview and commit share the one decision (ADR 0004). The fingerprint covers the alignment (left-aligned fingerprints are
   unchanged). A part of a styled selection (ADR 0012) keeps the whole selection's alignment.

## Consequences
- Left-aligned paragraphs keep their bytes unless the width tolerance applies (checked against a before/after dump of every
  fixture and corpus paragraph; see `CHANGELOG.md` 0.3.3).
- An edited right-aligned block keeps its right edge across edit, export, reopen and edit (round-trip test with PyMuPDF).
- HDDN can show the alignment in its editor from `align` and the box `[originX, originX + wrapWidth]`; it no longer needs to seed
  indents for aligned rows.
- Not covered: justified layout (word spacing), right-to-left text (still refused), rotated text (still refused).
