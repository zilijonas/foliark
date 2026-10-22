# ADR 0008: `invisibleText: 'keepOcr'` keeps scanner OCR and removes other hidden text (NEW-84)

Status: Accepted, 2026-10-22. Amends spec 1.6 and 8.2 (frozen: a change needs an ADR). Extends ADR 0006.

## Context
`keep` leaves every invisible (Tr 3/7) show in the file, so a hidden payload outside every region survives; `remove` fixes that but drops
the search layer of every scan (HDDN fixture hidden-ocr-text, NEW-82). A scanner's OCR layer is distinguishable by where it lies.

## Decision
1. New value `keepOcr`. An invisible show is kept iff every glyph box lies >= 80% over drawn raster image area, on every place the
   operator is drawn (4 x 4 samples per box against the union of image unit squares, clipped to the CropBox and the active clip;
   images hidden by optional content do not count). A shared Form operator is therefore decided once for all its instances.
2. Every other invisible show is replaced by an advance-only `TJ` like `remove`: over blank or vector-only area, straddling an image edge,
   outside the page, or without a place (singular CTM, size 0, Type3). Counted `removed.invisibleText` + `TEXT_SEARCH_LOST`.
3. Kept OCR is still ordinary text for rule 1: under a region it goes with the visible content. ActualText/Alt/E stay stripped (ADR 0006).
   `kept.invisibleTextClusters` counts what stayed. `keep` and `remove` are unchanged.

## Consequences
A hidden secret placed over a scan image looks like OCR and is kept; only a region removes it (limit of any placement test). A show
mixing in-image and off-image glyphs is removed whole. Unplaced invisible text is removed, not refused: its removal is disclosed
and nothing hidden is kept; unplaced visible text under regions still refuses `textPositionUnknown`.
