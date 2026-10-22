# ADR 0002: Native writer re-emits content streams from tokens

Status: Accepted, 2026-10-22 (NEW-24). Refines ADR 0001 decision 2 and spec §5, §8.1.

## Context
Redaction, editing and sanitization all end in "write a fresh file". Later tasks must rewrite single operators in a content stream, clone a Form, or replace an image with a re-encoded one, while everything else stays as it was. Two ways to produce a content stream were on the table.

## Decision
`foliark-engine::writer` builds a **fresh allowlisted object graph** on pdf-writer 0.15: new catalog and flat page tree, owned ids in discovery order (catalog 1, pages 2, pages 3..), only the keys of spec §8.1 per object class (tables in `writer/graph.rs`), classic xref, `%PDF-1.7`, no /ID, /Info, /Encrypt, /Metadata, object streams or prior revisions. Input encryption is read through and not reproduced.

**Every content stream is re-emitted from tokens** (page contents, Form, Type3 glyph procedures, tiling patterns): each instruction is copied from the byte span the reader reports, operand tokens verbatim (so `0.10` stays `0.10`), comments, junk between instructions and whitespace runs dropped, inline images (`BI`..`ID`..`EI`) kept as data, then Flate-compressed; the output must be a fixed point of re-emission, so a stream with a damaged inline image that cannot be delimited unambiguously is refused with `MALFORMED_PDF` instead of written (found by fuzzing, NEW-57).

A **substitution layer** is keyed by (stream ObjectId, byte span in the decoded stream): `replace_span` swaps every instruction a range touches for new operators; `replace_stream` replaces a whole object (cloned Form, re-encoded image; its dictionary still goes through the class allowlist); `append_page_content` wraps the page's content in `q..Q` and adds operators on top.

Stream data: content is decoded strict and capped, then re-emitted. Images, fonts and other data streams keep their bytes when the whole filter chain is allowlisted (Flate, DCT, JPX, JBIG2, CCITT); LZW, RunLength and ASCII chains are decoded (strict, capped) and Flate-compressed. An image filter behind a non-allowlisted filter is refused (no partial decode available).

Rules for the unknown: a reachable object whose class cannot be decided, a stream in a plain position and every dangling reference are typed `MALFORMED_PDF {objRef}` errors, never a skip. Hidden optional-content groups are refused (`OPTIONAL_CONTENT_UNSUPPORTED`) because dropping /OCProperties would show them; flattening is spec §8.2. The output budget is checked as objects are built and again on the written buffer (`LIMIT_EXCEEDED {maxOutputBytes}`).

## Rejected
- **Full re-synthesis from the semantic model.** Large surface (every operator, state, text mode), and any gap silently changes appearance. Fidelity risk with no benefit over spans.
- **Byte passthrough of content streams.** Comments, junk between instructions and unreferenced tokens can carry data a user believes removed; spans for later rewrites would also depend on untouched, unnormalized bytes.

## Consequences
- Output is deterministic for a given input and substitutions. Numbers in content stay exact; numbers in dictionaries pass through f32 (pdf-writer), which is exact for typical page geometry and weights.
- Annotations and widgets are not allowlisted. Until the flatten task draws them as Forms before writing, they are absent from the output.
- Deviations from the §8.1 key list: Image also keeps `Matte` (a soft mask renders wrong without it); fonts drop `CharSet` and `CIDSet`; ExtGState drops halftones.
