# ADR 0005: strict export rebuilds every retained font

Status: accepted (2026-10-22, lead, NEW-45)

## Context
Spec §5.6 and §8.2 rebuild only fonts touched by edits or redaction; untouched fonts pass through.
A passed-through TrueType program can still carry glyph outlines, names and tables for text that
no longer exists in the document (e.g. a subset made from a longer draft), so "untouched" is not
"clean".

## Decision
Native strict export rebuilds every retained font: ToUnicode limited to codes still drawn,
undrawn TrueType outlines emptied (gids stable), `name`/kerning/layout/vendor tables dropped,
`post` names kept only for drawn glyphs, unused Type3 CharProcs emptied. CFF/Type1 programs,
programs shared by several fonts and fonts used inside pattern cells are kept and listed in
`kept.fontsWithGlyphInventory` (`FONT_GLYPH_INVENTORY_RETAINED`). ICC streams are validated and
cut to their declared size; JPEG APPn (except Adobe), COM and trailing bytes are stripped.

## Consequences
Stricter than the frozen spec; output fidelity is unchanged (fidelity oracle) because PDF text is
positioned explicitly. Preserve-mode working copies (§8.3) are not affected.
