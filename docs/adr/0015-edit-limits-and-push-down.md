# ADR 0015: Evidence-based edit limits and opt-in push-down

Status: Accepted, 2026-11-01 (NEW-128; owner approved the design 2026-11-01). Implementation: NEW-129 (limits), NEW-132 (push-down). Amends spec §1.5 (DTOs), §4 (redaction and edit-rect coordinates), §6.2 (obstacles, `bottomLimit`, `maxWrapWidth`), §6.3 ("nothing below moves"), §12 (feature values) and §16. `wrapWidth` keeps today's value (ADR 0011, its lone-line amendment included); the extended width is a second, larger limit used only when the edit needs it. ADR 0012's segments are unchanged. ADR 0004 is unchanged: preview and commit share one `plan_edit`, and the fingerprint covers the reflow mode.

## Context
HDDN sample (ReportLab A4, non-embedded Helvetica 11 pt, text at x = 64): the paragraph "Your next invoice goes to Marta Jones… / Reply to marta.jones@example.com…" refuses `EDIT_NO_FIT` after 7 added characters, with about 77 pt free to its right and about 450 pt free below it. Causes in `edit/paragraph.rs` `prepare`:
- `wrapRight` (:608-618) is the page's largest visible text x when no word is beside the paragraph, else the paragraph's own max x. A short page has a narrow text extent, so the box ends where the longest line happens to end, not at the page's margin. Paths, images and shadings are not considered at all, so a table rule or a box edge never limits the width and never widens it either.
- `bottomLimit` (:619-629) is the top of the next word below in `[x0, wrapRight]`, else `H` minus the smallest visible text y. Spec §6.3 forbids moving anything, so a paragraph with text below it can never gain a line.

## Decision A: limits from the page's evidence, stable width first
Obstacles are the other visible words plus every painted (not clip-only) path, image and shading of the page, with display bounds as `redaction/plan.rs` `item_bounds` computes them (strokes grown by their width). Invisible and OCG-hidden content is not an obstacle (as today).
- **Container**: an item whose bounds contain the selection's bounds (a table cell box, a panel, a full-page background). It caps the right edge at its `x1` minus one space width and the bottom at its `y1`.
- **Decoration**: an item that overlaps the selection's x-range `[x0, x1]` and starts above `lastBaseline + descent` (underline, strike-through, highlight). It is not an obstacle.
- `leftMargin` = min x of the page's visible clusters. `free` (below) mirrors it: never less than what text on the page already reaches.
- `maxWrapRight = max(x1, min(free, nearest.x0 - space, container.x1 - space))`, where `nearest` is the nearest obstacle starting right of `x1` that vertically overlaps the selection `[top, y1]` (`y1` = the selection's bottom; terms that do not exist drop out). `space` is the space width of the paragraph's font, as today.
- `bottomLimit(r)` for a right edge `r` = the top of the nearest obstacle below the last baseline that overlaps `[x0, r]` in x, or the bottom of the nearest container, else `H - (min y of visible clusters)` as today; never above `y1`.
- `free = max(page max visible text x, W - leftMargin, other pages' max visible text x)`, the last term only when cheaply available (a per-revision cache of pages with the same display width, never interpreting pages per call). `free` never exceeds `W`.

Two widths, both on `ParagraphInfo`:
- `wrapWidth` (stable): today's value, `wrapRight` per spec §6.2 as before (page text extent, the paragraph's own max x with a word beside it, ADR 0011's lone-line rule) widened by ADR 0011's measurement; `maxLines` from `bottomLimit(wrapRight)`. Unchanged text re-breaks as on the page (ADR 0011's 0 re-breaks hold).
- `maxWrapWidth` (extended): `max(wrapWidth, maxWrapRight - originX)`; `maxWrapLines` from `bottomLimit(originX + maxWrapWidth)`. Both budgets keep ADR 0011's floor (never below the selection's line count).

Layout order, one decision shared by preview and commit (ADR 0004):
1. Lay out at `wrapWidth`; if every word fits and the lines are at most `maxLines`, done.
2. Else lay out at `maxWrapWidth`; if it fits in `maxWrapLines`, done.
3. Else, with `reflow: 'pushDown'`, push down (Decision B) keeping the `maxWrapWidth` layout, so the push is the smallest.
4. Else `EDIT_NO_FIT {neededLines, maxLines}` with the `maxWrapWidth` layout's figures (`maxLines` = `maxWrapLines`).

HDDN sample (expected; NEW-129's fixture confirms it): 7 characters added to line 1 need 3 lines at `wrapWidth`, over `maxLines` 2; at `maxWrapWidth` (the mirrored margin) they fit in 2 lines. `EditPreview.wrapWidth` reports the width the chosen layout used, so an editor widens its box only when the edit needs it.

## Decision B: opt-in push-down
`TextEdit.reflow?: 'none' | 'pushDown'` (default `'none'`, exactly today's behaviour). With `pushDown`, an edit that needs more than `maxWrapLines` lines at `maxWrapWidth` moves the content below it down by `dy` instead of refusing.

**Moved set.** `cut` = the paragraph's extended bottom limit, `bottomLimit(originX + maxWrapWidth)`. Start from every item (visible and invisible, OCR text and OCG-hidden content included) whose top is at or below `cut` and that overlaps the column `[x0, originX + maxWrapWidth]`; grow the column to the union x-range of the moved items and add what it now covers, until stable. Walking down by top, stop at the first vertical gap of at least `dy + 0.5 em` between the moved items so far and the next item: that item and everything below it stay (a footer below such a gap stays). `dy = (neededLines - maxWrapLines) * lineStep`. Containers of both the paragraph and moved items stay put.

**Bound.** The new paragraph's last line (baseline + descent) must end at or above `cut + dy`, and the moved block's new bottom must not pass `min(top of the next fixed item below it, bottom of any container it is in, H - (min y of visible clusters))`; else `EDIT_NO_FIT {neededLines, maxLines}` with `maxLines` = the push-down budget. Fixed items are everything not moved. Nothing leaves the page and nothing flows to the next page.

**Multi-segment paragraphs (ADR 0012).** A changed segment's `bottomLimit` is the next segment of its own paragraph, so push-down moves the rest of the paragraph (and what follows it) down, not only the content after the paragraph.

**Mechanism.** Whole units are translated by `cm` operators inserted through `Writer::replace_span` (`writer/mod.rs:205`; the unit's first and last operator are replaced by themselves plus the `cm`), in the same writer pass as the edit:
- a `q … Q` group whose painted items all move gets `1 0 0 1 tx ty cm` right after its `q` (outermost such group wins);
- a leaf unit (`BT … ET`; a path object from its first `m`/`re` to its paint operator; `Do`; `sh`; `BI … EI`) gets `cm` before and the inverse `cm` after. No `q`/`Q` around leaves: `Tf`, colour and `gs` set inside a leaf carry over to later content.
- `(tx, ty) = L⁻¹ · D · (0, dy)`, `L` = the linear part of the CTM at the insertion point, `D` = the linear part of `displayToUser`, so `/Rotate`, `/UserUnit` and scaled streams are handled. Numbers use `redaction/text.rs` `num(v, 5)`; the inverse uses the same strings negated. Output stays deterministic.
- A clip-only path, or text in a clipping render mode (4-7), moves only with its whole enclosing `q … Q` group (its clip outlives the leaf).
- Link annotations wholly over moved content get `/Rect` and `/QuadPoints` shifted by `dy` in the working copy (a preserve-mode entry override in the writer, `Writer::override_entry`; `Writer::supersede` with a rebuilt dictionary is the existing alternative for indirect annotations).

**Refusals** (typed, no partial output): `EDIT_NO_FIT {neededLines, maxLines, feature}` with `feature`:
- `reflowSharedBlock`: one `BT … ET`, path object or `q … Q` leaf mixes moving and fixed items;
- `reflowSplitsForm`: a Form occurrence holds both moving and fixed items;
- `reflowStraddle`: an item crosses `cut` and is not a container of both the paragraph and the moved items;
- `reflowClipped`: a moved unit's clip, set outside the unit, is not an axis-aligned rectangle containing the unit's shifted bounds;
- `reflowSoftMask`: a moved item is drawn under an ExtGState soft mask;
- `reflowPattern`: a moved item is painted with a pattern colour (pattern space is anchored to the parent's default space, so `cm` would slide the shape over its pattern);
- `reflowSharedContent`: a stream that would get `cm` is drawn by another page or another occurrence;
- `reflowAnnotation`: a non-Link annotation, any widget, or a Link only partly over moved content meets the moved area.
A plain overflow of the bound has no `feature`.

**Wire.**
```ts
interface TextEdit { ...; reflow?: 'none' | 'pushDown' }
layoutEdit(page, rects, text, o?: { font?; glyphs?; reflow?: 'none' | 'pushDown' }): Promise<EditPreview>
interface PageShift { pageIndex: number; region: Rect /* display space, before this step */; dy: number }
ParagraphInfo.maxWrapWidth: number; ParagraphInfo.maxWrapLines: number   // extended width and its budget
ParagraphInfo.pushDown: { maxLines: number; region: Rect } | null   // budget with push-down at maxWrapWidth, the moved region at that budget; null = impossible
EditPreview.wrapWidth: number   // the width the chosen layout used
EditPreview.shift: PageShift | null; EditPreview.reflowBlocked?: string   // the refusal feature when pushDown is impossible
ChangeResult.remap: PageShift[]   // ordered old -> new steps for overlays (redaction boxes, highlights)
```
`remap` undoes the old working copy's shifts that differ (in reverse order: region moved by `dy`, `-dy`) and applies the new ones in edit order; a shared prefix of identical shifts cancels, so a redaction-only ChangeSet has `[]`. The fingerprint covers the reflow mode and the width used.

**Later edits' rects.** The contract stays: each edit's `rects` are in the coordinates after the edits before it, as of the previous applied revision. An edit whose id was in the previous list is in the frame after that list's edits before it; a new id is in the base-revision frame (after all of them). The session keeps each committed edit's shift; while replaying, before edit `k` selects, the engine maps its rects from that old frame to the frame after the new edits `0..k-1`, with the same steps as for redactions (old shifts that differ undone, new ones applied, identical prefix cancels). A rect wholly inside a step's region is translated, one outside it is untouched, one partly inside refuses `INVALID_ARGUMENT {feature: "editTargetShifted", editId, pages}`, nothing changes. Without push-down every step list is empty and nothing changes from today. Consumers still get `ChangeResult.remap` for overlays.

## Redactions (privacy-critical)
Redactions are always stated in base-revision coordinates (spec §4): the working copy the caller saw. When `applyChanges` replaces the edit list (`Session::apply`, `document.rs:259-384`), the engine maps every stored and newly submitted redaction, region and text alike, from the old working copy to the new one through the `remap` steps. Region redactions are not remapped today; they are from now on.
- A region rectangle wholly inside a step's region is translated by `dy`; one partly inside is refused, `INVALID_ARGUMENT {feature: "redactionTargetShifted", pages}`, nothing changes.
- A text redaction's must-remove clusters are found again (`remap`, `document.rs:404-450`, via `reresolve` :454-468) by comparing each old glyph box mapped through the same steps, at the existing 0.05 pt; its regions are mapped like region redactions.
- A region left in the band the moved content vacated now covers the edited paragraph's new lines: over-redaction, never a leak.

## Verification (required with the implementation)
- Oracle (`tests/oracles`, never importing the engine): the moved region `R` of the input renders equal to `R + dy` of the output, and everything outside the column and band is unchanged. Negative control: an output where one moved item stayed behind (or moved by `dy + 1`) must fail.
- Redaction in base coordinates: a region drawn over content that a later push-down edit moves; export; `objects.py --forbid` the moved secret. Negative control: the same export with the redaction left at its old rectangle must be caught.
- Determinism: export twice, equal sha256.

## Options rejected
- Push-down on by default: re-lays out existing replayed ChangeSets and moves content apps did not expect; opt-in keeps old ChangeSets byte-identical.
- Splitting a straddling redaction (or edit rect) into a moved and a fixed part: correct but more cases to prove; the caller redraws it. A possible later step.
- One extended width for every layout: simpler, but it re-breaks unchanged soft wraps beside a column or under a mirrored margin, losing ADR 0011's 0 re-breaks.
- Shrink-to-fit or reflow onto the next page: spec §0.1 forbids the first; the second rewrites page structure (later, §16).

## Consequences
- `wrapWidth` is unchanged, so text that fit before lays out exactly as before. An edit that refused `EDIT_NO_FIT` may now fit at `maxWrapWidth` (wider lines than the page's); `maxLines` can drop where a path, image or container below now counts as an obstacle. Record both in `CHANGELOG.md` with NEW-129.
- Under `'pushDown'` the page's content below an edit moves; consumers re-place overlays with `remap`.
- `prepareParagraph` computes the push-down budget (one more pass over the page's items, no I/O).
