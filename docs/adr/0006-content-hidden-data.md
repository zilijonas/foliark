# ADR 0006: Content streams carry no hidden data (NEW-59)

Status: Accepted, 2026-10-22. Amends spec 8.1/8.2 (frozen: a change needs an ADR).

## Context
Independent acceptance (docs/evidence/security-acceptance.md) found two channels the allowlist writer did not cover because they live inside
content streams, which are re-emitted token by token (ADR 0002): (1) inline marked-content property dictionaries (`BDC`, `DP`) kept every key
except ActualText/Alt/E, so `/Contents (secret)` or any private key survived sanitization; (2) instructions with an unknown operator (legal inside
`BX..EX`, ignored by readers) were copied with all operands.

## Decision
1. The re-emitter drops every instruction whose operator is not in PDF 32000 table 51, with its operands.
2. An inline `BDC`/`DP` dictionary keeps only `/MCID`, `/Type`, `/Subtype`, `/BBox`, `/Attached`, and only when the value holds no string or
   dictionary. Named property lists (`/Tag /Name BDC`) are unchanged; `/Properties` is sanitized with the resources.
3. Both apply to native export and to preserve-mode working copies (same re-emitter). Rendering and text extraction do not read the dropped data.

## Consequences
`/Lang` and `/K` entries in inline property lists no longer survive (accessibility metadata is already removed with the structure tree, spec 8.2).
A producer that relies on an unknown operator for private rendering loses it; no conforming reader draws anything for it.
