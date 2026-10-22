# ADR 0007: Native export keeps only resources used by surviving content (NEW-80)

Status: Accepted, 2026-10-22. Amends spec 8.1 (frozen: a change needs an ADR) and ADR 0006.

## Context
The allowlist writer is reachable-only, but a `/Resources` entry no operator names is still reachable. Independent acceptance (case `unused-xobject`,
`unused-font`) found Forms, images, fonts, patterns and shadings that content never draws written out with their full streams, in sanitize and in
redaction exports: invisible to text extraction, visible to a stream scan. Editors leave replaced content there.

## Decision
1. The last native stage (after redaction and fills, on the written file read back) collects the names surviving content uses and drops every
   other entry of `Font`, `XObject`, `ExtGState`, `ColorSpace`, `Pattern`, `Shading` and `Properties` (`sanitize::prune`), by a second write.
   What only those entries reached is unreachable and not written. Nothing visible changes.
2. Names come from `Do Tf gs sh cs CS scn SCN BDC DP` and inline image colour spaces. Used Forms, tiling cells, Type3 glyph procedures and
   soft-mask groups are scanned transitively. An owner without its own `/Resources` (Form, cell, Type3 font) inherits the drawing context's, so
   its names count there. A colour space named inside a used colour space array stays.
3. Reported as `removed.unusedResources` (entries dropped); no loss code. Preserve-mode working copies and raster mode are untouched.
   `fontsWithGlyphInventory` lists only fonts still in the file.

## Consequences
A producer relying on a Form falling back to the page's resources for a name its own `/Resources` lacks loses that name if the page does not
use it. Prune failure (undecodable content, nesting limit) refuses the export; it never skips silently.
