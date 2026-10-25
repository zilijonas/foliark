# ADR 0013: An edit keeps inline bold and italic runs

Status: Accepted, 2026-10-24 (NEW-113); amended 2026-10-25 (NEW-115, see Amendment). Amends spec §6.2 (`ParagraphInfo.looks`, `runs`), §6.3 (each character drawn in its own run's style) and §6.4 (a caller font per run); amends ADR 0012 (a style change mid-line is no longer flattened). ADR 0004 is unchanged: preview and commit still call one `plan_edit`, and the fingerprint covers the runs.

## Context
Owner's Word law PDF in Safari on 0.1.3: editing one word of article 2 turned its inline bold defined terms (bold runs inside regular body lines, Arial-Bold/Arial twins) regular. ADR 0012 splits a selection only at hard breaks where the row's look changes; inside a segment the whole changed part was laid out with one ladder, the segment's main style, so every bold or italic glyph on the re-laid-out lines came back regular. With HDDN's "Use a similar font?" the caller font (one registered face) drew the whole part. On the article the owner edited, 14 inline bold terms (381 bold glyphs) sit in a region of 6 segments; 3 of the terms share the edited segment.

## Options
- (a) Split segments at every style change. A run boundary mid-line has no line of its own to keep: the rows still re-break as one paragraph.
- (b) Keep per-character styles inside one layout, carried through the edit by a text diff (ADR 0012's option (b)).
- (c) Refuse edits of mixed lines. Safe, but the owner's law documents are mostly mixed lines.

## Decision
(b), inside ADR 0012's segments:
- Looks: the page styles the selection is drawn in (`StyleRun` index; Word's twins are two looks of one face), the paragraph's own first, each with its font as rung 2 built from that font's glyphs on the page. Every character of `originalText` has a look: a cluster's own; a word gap, a soft-wrap space, a hard break and an indent, which the page does not draw, the paragraph's.
- Carrying runs (`edit/runs.rs` `carry`): the edited text (normalized) is lined up with `originalText` word by word: common prefix and suffix of words, then the longest common subsequence of the words between (at most 2^20 word pairs; above that the middle is one replaced block). A shared word keeps its characters' looks. In each replaced block a common character prefix and suffix keep theirs; then text that replaces text takes the look of the first character it replaces, inserted text touching a word takes that word's look (the character before it first, so typing at the end of a bold term stays bold), an inserted whole word takes the look its neighbouring words share or else the paragraph's, and new whitespace takes the paragraph's (it draws nothing, and a bold subset often has no space).
- Layout (`foliark-text` `layout_runs`): one paragraph, one greedy breaking and budget, with `TextRun`s: each run of characters has its own ladder, size and colour; a cluster resolves through its run's ladder only. `emit_plan` sets each run's fill. The segments of ADR 0012 are unchanged; the changed part is laid out with its slice of the carried looks, and the looks stay the whole selection's.
- Caller font (`TextEdit.font`, HDDN's similar-font mode): it draws the runs in the part's main weight and style. A run in another weight or italic takes the registered face of the caller's family with exactly that weight (400/700) and style. If that face is not registered the edit refuses `EDIT_NO_FONT` with the characters that need it; it never draws them in the caller's face.
- ADR 0011's measured box lays every original row out with its runs' fonts.
- Wire: `ParagraphInfo.looks` (`TextLook`: size, colour, flags, the page's font, editor face) and `ParagraphInfo.runs` (`StyleSpan` UTF-16 ranges tiling `originalText`, each a look index); `EditPreview.styles`: the carried looks of the whole text sent, as spans over the same looks. An editor draws each span in its look's face.

## Consequences
- Synthetic Word fixture (`word-inline-runs`, inline bold with a Type0 twin letter and an inline italic reference): editing a regular word, inside a bold term and across a run boundary keeps every glyph in its run's face (judged glyph by glyph on the extracted commit), preview fingerprint equals the commit's, objects and fidelity oracles pass on the export.
- Owner law PDF, article 2, one 10-letter body word changed (counts only): 14 of 14 inline bold terms keep their bold glyph counts in the commit and in the export (381 bold glyphs before and after), 3 of them on the re-laid-out lines; rendered ink per term 0.995 to 1.009 of the input. Owner iOS PDF: 41 extraction blocks, 0 refuse, 0 overflow, 0 re-break with the text unchanged.
- A look whose characters no font covers still refuses `EDIT_NO_FONT` when it is laid out; a run's own source subset covers only the characters the page draws with it, so a letter new to a bold term falls to the registered fonts (the bold face by flags) as before.
- Size per run is kept; a superscript's rise is not (it lands on the baseline), as before.
- Limit: a run boundary inside a word that a replaced block covers goes with the block's rule (the first replaced character's look), e.g. retyping a whole word whose first letter was bold.

## Amendment, 2026-10-25 (NEW-115): retyped text takes one look
Owner's Word PDF in Safari: select all in a paragraph (regular start, inline bold, italic tail) and type a three-word sentence ending in a full stop gave a regular first word and italic next words; select from a bold word to the end and type gave bold, then regular, then italic. Cause, measured on that paragraph (look ids and token positions only): the word diff treated each single whitespace character as a token that can match, so the new text's two spaces lined up with the last two spaces of the old italic tail, which cut one replaced block into word-sized pieces, and each piece took the look of the old word its space landed on; the final '.' kept the old italic as a common character suffix.

Rule, replacing "a shared space aligns words" and "a common character prefix and suffix keep their looks in any block":
- Only words line up (prefix, suffix and the longest common subsequence run on words); a shared whitespace character is no evidence that the words around it correspond.
- Between shared words, old and new text with the same number of words (two or more) replace word for word, each new word taking the look of the old word it replaces (`Owner is` to `Owners be` keeps bold on the first word only). With any other counts the block is one replacement and takes ONE look: the first replaced character's. The insertion rule (neighbours' shared look, else the paragraph's) applies only where nothing is replaced.
- In a block, equal whitespace at its ends keeps its looks; a common character prefix and suffix keep theirs only when each side is a single word (an in-word edit: a twin letter, a letter typed at the end of a word), since across several words a shared letter or full stop is stray.
- A new whitespace character is still the paragraph's.

Consequence: `Owner is` replaced by `Owners now be` is one block and is all bold; before, only `Owners` was (the fixture tests now retype `Owner is` as `Owners be`, word for word). Typing a letter new to a bold run over a bold term still needs the bold face (`EDIT_NO_FONT` as above). A word kept by the diff keeps its characters' looks as before.
