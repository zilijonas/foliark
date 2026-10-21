# ADR 0001: Foliark v1 core contract and engine stack

Status: Accepted, 2026-10-22 (NEW-3). Normative detail: `docs/spec/v1.md`.

## Context
HDDN redacts and edits PDFs in the browser through Pyodide + PyMuPDF 1.26.3 (~29.6 MB of assets, AGPL/commercial licensing, partial sanitization that refuses forms, optional content and invisible text). Foliark replaces it with a headless Rust/WASM SDK that HDDN and later consumers load from static hosting, offline and on mobile. The NEW-1 baseline lists every PyMuPDF capability HDDN uses; the NEW-4 cohort spike verified hayro 0.8, pdf-writer 0.15, subsetter, harfrust/skrifa and measured the wasm at 5.6 MB raw / 1.81 MB gz (1.24 MB gz without embedded fonts and CMaps). The spike found four blocking gaps in stock hayro: no operator byte spans or Form boundaries, no raw char code or font ref, hidden optional content never reaches the device, and decode failures come back as empty streams.

## Decision
1. **Parse, interpret, render: hayro 0.8**, vendored as a narrow fork under `vendor/` (hayro-syntax, hayro-interpret, hayro renderer) through `[patch.crates-io]`. Each change is logged in `patches/hayro/README.md` and is additive and upstreamable: operator byte spans + content-stream identity, Form XObject begin/end events, raw char code + font ObjRef getters, OCG override, strict decode mode, plus a repaired-xref flag.
2. **Writer: our own serializer on pdf-writer 0.15** that emits a fresh, reachable-only graph from an allowlist, with a substitution map for replaced streams/images and byte-exact pass-through of untouched streams. Output is never encrypted, never appended, and has no orphans or prior revisions. hayro-write is not used.
3. **Editing layout: harfrust 0.14 + skrifa 0.46** (the same skrifa as hayro) plus our own Latin LTR greedy line breaker. Rust's LayoutPlan is authoritative for preview and export.
4. **Subsetting: subsetter 0.2.6.** Fallback fonts are registered by the caller (`registerFonts`); the SDK ships none. hayro's std-14 substitutes are embedded in the wasm because extraction metrics and rendering of non-embedded base fonts need them synchronously; predefined CMaps are an optional lazily fetched pack.
5. **Runtime:** one single-threaded module worker per SDK instance, wasm-bindgen facade, cooperative step slicing with a supervisor that hard-terminates and restarts, several open documents per worker. No SAB, COOP/COEP or OffscreenCanvas.
6. **Scope:** every PyMuPDF behavior HDDN uses has a v1 equivalent; v1 closes HDDN's gaps where feasible (flatten forms/annotations/optional content, keep and redact invisible OCR text, destructive image and vector redaction). Everything else is a typed capability refusal, never a silent behavior. Where richer semantics would strain a small team, v1 picks the simpler safe rule and the spec lists the richer one under "Later".

## Alternatives rejected
- **PDF.js + pdf-lib**: two independent parsers (render vs write) disagree on hostile input; pdf-lib cannot remove text from content streams or rewrite images, so redaction would stay overlay or raster-only. JS-only performance and memory on mobile are worse for 30-page exports.
- **MuPDF or PDFium compiled to WASM**: C/C++ engines are excluded from the shipped graph (license: MuPDF is AGPL/commercial, which is the business driver for Foliark; memory safety on hostile input; build complexity). They stay as dev-only oracles.
- **Parley 0.12**: pulls fontique, ICU data and a second skrifa/read-fonts pair (~70 crates) for capabilities v1 does not use (bidi, complex scripts, rich text). Fixed-size Latin paragraph layout is a few hundred lines on harfrust.
- **hayro-write**: self-described internal crate; it re-synthesizes /Contents from the merged decoded page stream with no substitution hook, drops catalog-level data, skips objects that fail to load and has an open TODO for crypt filters. Usable as a reference only.

## Consequences
- We own a fork: rebase cost on each hayro minor and a duty to keep patches small and offer them upstream. Pins are exact (`=0.8.0`).
- The contract is deterministic, revision-bound and refusal-first, so HDDN's adapter is mostly mapping (spec §15), and the pdf-lib flattened path is deleted.
- v1 vector redaction removes whole crossing subpaths (except exact axis-aligned rectangle cuts) and discloses the collateral; fonts with CFF/Type1 programs keep their glyph inventory, also disclosed. Both are listed for owner review and as "Later" upgrades.
- Revisions are materialized by re-writing the working copy (simple, costs a write per accepted ChangeSet); an XRef overlay is a later optimization.
- Payload target: about 1.5 MB gz wasm (std-14 embedded, CMaps optional) versus PyMuPDF's 29.6 MB; this is unmeasured until the first real build and must be confirmed before any size claim.
