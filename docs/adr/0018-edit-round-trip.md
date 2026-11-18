# ADR 0018: a written file stays editable; removed stretches lose their spaces; edits read back (NEW-138)

Status: Accepted, 2026-11-19. Amends spec 5.1 (text removal), 6.3 (apply) and 12 (`INTERNAL` row).

## Context
HDDN saves an edited invoice (0.3.1) and the user opens and edits the saved file. Three defects showed:
- Resource names this engine writes (`FkE{n}S{k}` for a reused source font, `FkR{tag}` for a registered font, `FlkImg{n}`,
  `FlkFm{n}`, `FoliarkFlat{n}`) were fixed. The saved file already carries them, so the next edit or redaction on the same page
  failed with `INVALID_ARGUMENT` ("resource name in use").
- A registered font was found again by its resource name. Export strips the private `/FoliarkGlyphs` record (it carries edit
  text), so on the saved file the name led to an old subset with no record: the new subset was embedded, the page was not given it,
  and the new CIDs were drawn through the old 3-glyph font. The line read as garbage; nothing refused.
- Removal took the clusters of the removed words only. Space glyphs between them stayed at their old x, so an edited line or a
  set of per-word redactions still told where the removed words began and ended.

## Decision
1. Every resource name a writer adds is allocated fresh: the base name, else `base_2`, `base_3`, ... when the page's own or
   inherited resources (a Form's own, for a Form) or an earlier addition already use it. The writer returns the name; callers draw
   with it. Base names are unchanged, so output for files without such names is byte-identical.
2. A registered font is reused only when its record is proven: `/FoliarkGlyphs` present, the `BaseFont` subset tag of the font
   id, and one record entry per CID of `/W`. Anything else is foreign: the new subset gets its own fresh name on the page, and the
   old object is never extended, superseded or drawn through.
3. Spec 5.1 addition: a whitespace cluster goes with the removed clusters when its non-whitespace neighbours on its line
   (before and after, where there is one) are all removed (between removed words, or leading or trailing a removed run), or when
   its centre lies in the box of a run of removed clusters on a line (a space an earlier edit left behind, drawn on its own). A
   space beside a kept glyph stays, so kept words keep their spacing. This applies to edits and redactions alike; it removes
   nothing visible.
4. Spec 6.3 addition: after writing, an edit reads its new text back from the page's last content stream (the one it appended)
   and compares it with the planned text (NFKC, whitespace dropped, U+00AD/U+2010/U+2011 read as `-`). A mismatch rejects the
   edit with `INTERNAL`, `details.feature: 'editReadBack'`, and nothing changes. It is a bug detector, like `REDACT_VERIFY_FAILED`;
   it arrives as a `RejectedChange`, not a thrown error, so the client is not reset.

## Consequences
- Files saved by 0.3.1 become editable and redactable again without migration.
- Removal counts (`clustersRemoved`) grow by the spaces inside removed stretches.
- Each edit reopens its output once (it was reopened for the next edit already) and interprets the edited page once more.
- A font whose ToUnicode maps a drawn glyph to another character than the one typed (beyond the folds above) is refused rather than
  written with text that reads differently.
