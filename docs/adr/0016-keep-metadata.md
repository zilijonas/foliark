# ADR 0016: `metadata: 'keep'` writes the standard Info fields back on native export

Status: Accepted, 2026-11-01 (NEW-128; owner approved the design 2026-11-01). Implementation: NEW-134. Amends spec §1.6 (`ExportPolicy`, `KeptReport`), §8.1 (no `/Info`) and §8.2 (Info row). Extends ADR 0002 (allowlist writer); reuses ADR 0014's token rule.

## Context
Native export removes the document information dictionary and every XMP packet (`METADATA_REMOVED`). Title and author are useful in a redacted file (document lists, PDF viewers' title bar), and the owner wants them back as an explicit choice. Metadata is also where a redacted name survives most often: a title "Contract with Marta Jones", an author, keywords, and XMP packets that can carry edit history and thumbnails of the unredacted original.

## Decision
`ExportPolicy.metadata: 'remove' | 'keep'`, default `'remove'` (today's behaviour, byte for byte). Native export only: raster export never writes Info (spec §7), whatever the policy. Preserve-mode working copies are unaffected (they keep Info verbatim, as before). The policy hash changes only when `keep` is set.

With `keep`:
- Only these Info keys can be written: `Title`, `Author`, `Subject`, `Keywords`, `Creator`, `Producer`, `CreationDate`, `ModDate`. Every other key (custom keys, `Trapped`) is dropped. Values come from hayro's parsed `Metadata` (`vendor/hayro-syntax/src/metadata.rs`, raw bytes and `DateTime`) of the working copy, never from the raw objects.
- Text fields are normalized: decoded as UTF-16BE (BOM `FE FF`), UTF-8 (BOM `EF BB BF`) or PDFDocEncoding (undefined codes dropped); language escape sequences (`U+001B … U+001B`) removed; NFC; C0/C1 control characters and the bidi controls of spec §6.1 removed; surrounding whitespace trimmed. An empty result, or one over 1024 characters, drops the field (no truncation). A value is written as a literal string when it is printable ASCII, else as a UTF-16BE hex string with BOM.
- Dates are re-emitted in the `D:YYYYMMDDHHmmSS` grammar with the parsed offset (`Z` for zero, else `+HH'mm'`/`-HH'mm'`); a date that does not parse or is out of range drops the field, and so does an offset under one hour that is not zero (hayro's parsed `DateTime` loses the sign of such an offset, and a wrong sign is not written).
- A text field is dropped when its normalized lowercase value, or its letters-and-digits-only form, contains a token of text taken out of the document: by a redaction (`sanitize/links.rs` `removed_tokens` over the requested and effective regions, as for links) or by an edit (the words of the edited pages in the source that no edited page of the working copy still shows, so a word an edit kept is not lost; `sanitize/metadata.rs` `edited_away`). Tokens are compared in both forms too. Same rule and over-dropping as ADR 0014: a leak is not accepted.
- The fields are written fresh by the allowlist writer as one new dictionary, keys in the order above, referenced from the trailer `/Info`. Output stays deterministic. No `/ID`.
- XMP is always removed: the catalog `/Metadata` and `/Metadata` on any object, with or without `keep`. It is the field most likely to hold history, thumbnails and private namespaces, and it duplicates the Info fields.

Report: `kept.metadata: string[]` lists the key names written, in the order above (`[]` under `remove`). Under `keep`, `removed.metadata` and `METADATA_REMOVED` count the XMP packets plus every Info key not written (dropped fields and other keys); under `remove` the count is as today.

## Verification (required with the implementation)
- `tests/oracles/objects.py --allow info-standard`: an `/Info` dictionary is accepted only with those 8 keys, string values, and `D:` dates for the two date keys; the existing `--forbid` scans (strings and streams) still run over it.
- Negative controls in `controls.py`, both must be caught: `leak_info_redacted_title` (a Title spelling a forbidden token) and `leak_info_custom_key` (a custom Info key under `info-standard`).
- Fixtures: a redacted name in the Title drops the Title; an edited-away name in Keywords drops Keywords; non-ASCII titles round-trip; XMP present in the input is absent in the output.

## Options rejected
- Keep XMP (or regenerate it from Info): no consumer needs it, and a fresh packet is more surface for the same 8 fields.
- Keep any Info key with a string value: custom keys are where producers put file paths, user names and company data.
- Truncate long values: changes the content silently; dropping is reported.

## Consequences
A title that spells redacted or edited-away text is lost on purpose and counted. A field that spells removed text only in a form no token matches (a different spelling, an abbreviation) is kept: the check is a text test, not a proof, as for links. `Producer` keeps the original producer; Foliark does not stamp itself.
