# ADR 0011: Unchanged text fits its own edit box

Status: Accepted, 2026-10-23 (NEW-104). Amends spec §6.2 (`wrapWidth`, `maxLines`) and §6.4 (cluster coverage); refines ADR 0003. ADR 0004 is unchanged: preview and commit still share one layout path and fingerprint. ADR 0012 measures the box without the caller's font and per style segment.

## Context
On the owner's iOS PDF, 10 of 38 paragraphs reported "does not fit" with their text untouched and 11 re-broke differently; one more refused `EDIT_NO_FONT`. Measured (counts only):
- The source font was reused (rung 2) and laid out with its `/W` widths, as the content stream lists them. The page draws narrower: 235 of 1055 consecutive glyph pairs carry a TJ correction of exactly -1/1000 em, the same for every occurrence of a code. No Tc, Tw or Tz. `wrapWidth` is the drawn extent, so the last word of a full line, or a one-word box, came out 0.01 to 0.05 pt too wide against a 1e-6 pt epsilon.
- Two paragraphs got `maxLines` 0: the word below starts above the paragraph's own descent, so the budget held fewer lines than the paragraph already has.
- The ogonek is drawn as `a` plus a zero-advance U+0328 (no kern; the mark's outline sits left of its origin). NFC makes it `ą`, which the subset lacks.
- The fixture corpus shows a second cause: 18 of 72 untouched paragraphs did not fit, all one-word lines in non-embedded Base-14 Helvetica laid out in the Liberation Sans fallback, whose 2048-unit advances are 0.007 to 0.014 pt wider over a line.

## Options
- (a) Lay out with the advances the page draws: learn per code (or per pair) the TJ/Tc/Tw-adjusted step from the page and write the same corrections back. Exact for a reused source font, but it needs a threshold to tell a rounding correction or kern from a positioning jump (the owner file has jumps of 1.3 to 17.9 em inside one TJ), and it cannot reach the fallback-font case without changing the fallback policy (out of scope).
- (b) A fixed tolerance on the width test. Arbitrary, and it lets new text overflow by that amount.
- (c) Measure the box with the layout itself.

## Decision
(c):
- `wrapWidth = max(wrapRight - originX, widest original line as the layout measures it)`. Each line of `originalText` (soft breaks included) is laid out as its own hard line, with the same ladder, size and indent rule an edit gets; its extent is indent plus advances. The tolerance is the paragraph's own measured excess, nothing fixed. A paragraph without `originalText` (unmapped clusters) or whose text the layout refuses keeps the spec value.
- `maxLines` is never below the number of lines the selection has now.
- A cluster no candidate covers in NFC is tried as its canonical decomposition, per rung, before the next rung: a source subset holding `a` and U+0328 draws `ą` as those two glyphs and stays exact (not approximate). Both glyphs map to the whole cluster (UTF-16 range of `ą`), like a ligature in reverse. The reverse case needs no rule: text is NFC, so a letter whose precomposed form exists arrives precomposed.

## Consequences
- Unchanged text fits and re-breaks as on the page: owner file 0 of 38 refusals, 0 changed breaks (with and without registered fonts); corpus 0 of 79 (was 18 of 72).
- New text is measured with the font that draws it: the source font's `/W`, a registered font's own advances, as before. Nothing is learned from the page, so a character not on the page (no `/W` evidence) falls to the next rung exactly as before.
- The box may exceed the page extent by the original text's own excess (0.01 to 0.05 pt here). Edited text can reach that far, never further: text longer than the widest original line, as measured, still wraps or refuses `EDIT_NO_FIT`. `ParagraphInfo.wrapWidth` reports the widened value, the one the layout uses.
- A decomposed mark is written with its own advance and no kern, as Quartz draws it. A source font whose marks need a positioning kern (outline right of the origin) would show the mark beside its base; no such file is known.
- `prepare` lays the original text out once more (one shaping pass, no I/O).
