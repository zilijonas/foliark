# ADR 0014: `links: 'keep'` writes safe hyperlinks back on native export (NEW-124)

Status: Accepted, 2026-10-29. Amends spec 1.6 (`ExportPolicy`) and 8.2 (annotations; frozen: a change needs an ADR). Extends ADR 0002 (allowlist writer).

## Context
Native export removed every link (`LINKS_REMOVED`). A redacted contract that loses its table-of-contents jumps and its web references is
harder to use, and the owner wants them back as an explicit choice. A link is not content, but it can carry content: a `mailto:` or URL
that spells the redacted name, an action that runs code or reaches out, a rectangle that marks where redacted text was.

## Decision
Owner decision, fixed: an opt-in `ExportPolicy.links: 'remove' | 'keep'`, default `remove` (today's behaviour, byte for byte). Native
export only; raster export removes links as before. Comments, accessibility tags and signatures stay removed and get no option.

With `keep`, a `/Subtype /Link` annotation is kept only when all of these hold:
1. Its action is a `URI` (`http:`, `https:` or `mailto:`, scheme case-insensitive; printable ASCII, at most 2048 bytes), or a `GoTo` `/D`
   or a link `/Dest` that resolves to an explicit destination array whose first element is a page of the output and whose view
   (`XYZ`, `Fit`, `FitH`, `FitV`, `FitR`, `FitB`, `FitBH`, `FitBV`) has its argument count. A named destination (name or string) is looked up
   in the catalog `/Dests` dictionary or the `/Names /Dests` name tree; if it does not resolve the link is dropped. The output always holds
   the explicit array: Names and Dests are not written.
2. It has no `/Next`, no `/AA`, no `/OC`, and is not Hidden or NoView (the fresh dictionary cannot carry them, so a link meant to be inert
   would become live). JavaScript, Launch, GoToR, GoToE, SubmitForm, ImportData, Named and every other action drop the link.
3. Its `/Rect`, and every `/QuadPoints` quad, do not overlap any redaction region of its page, in display space with the overlap test of the
   redaction planner, against both the requested and the effective (grown to whole clusters, images, vectors) regions.
   The same holds for an edited paragraph: its old rectangles and the area of its new text (the whole page when the new text cannot be
   found), so a link to an address the user edited away does not survive.
4. A URI, percent-decoded (repeatedly), NFC-normalised and lowercased, holds no token of the redacted text. Tokens: each
   whitespace-separated piece of at least 3 characters (NFC, lowercase) of the text of every cluster the regions take, anywhere in the
   document, plus each region's text with its whitespace removed. Over-dropping is accepted; a leak is not.
5. Its page survives (every page does).

A kept link is written fresh from an allowlist, never copied: `/Type /Annot /Subtype /Link /Rect`, `/QuadPoints` (numbers, a multiple of 8),
`/Border [0 0 0]`, `/H` (N, I, O or P), and either `/A << /S /URI /URI (...) >>` or `/Dest [pageRef view ...]`. No `/Contents`, `/NM`, `/M`, `/F`,
`/AP`, `/StructParent`, `/OC` or `/P`. The page `/Annots` holds only these. A link is never drawn into content. Each is its own object
(never merged by object dedupe), and the unused-resource rewrite writes them again.

Report: `kept.links` counts the links written; `LINKS_REMOVED` counts the others and appears only when at least one was dropped;
`removed.annotations` no longer counts the kept ones. The policy hash changes only when `keep` is set.

## Why not signatures, comments, accessibility
A signature covers the bytes of the file; redaction changes them, so the signature cannot stay valid, and keeping it would need an
incremental update, which leaves the unredacted original recoverable in the file. Comments carry text by design (`/Contents`, author,
replies) and accessibility tags carry `/Alt` and `/ActualText`: each needs its own content rules and is a separate decision, so neither
gets an option here.

## Consequences
Links under a redaction, or spelling its text, are lost on purpose and reported. A URL that spells redacted text only in a way no token
matches (a different spelling or a transliteration) is kept: the check is a text test, not a proof. Oracle: `objects.py` spec `links`
(allowlisted keys, URI schemes, no token, no region overlap, destinations on pages of the file), with leaky controls in `controls.py`.
