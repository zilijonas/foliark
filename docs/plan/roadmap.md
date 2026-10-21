# Foliark delivery roadmap — planning baseline, 2026-10-21

## Goal
Deliver independently reusable, headless Rust/WASM PDF SDK with HDDN as first consumer. Finish with an installable npm package and verified HDDN workflows for inspect, text selection/search, render, paragraph editing, permanent redaction, strict sanitization, explicit raster export, cancellation and offline/mobile use. This board is a plan: no SDK implementation, benchmark result, legal clearance or release approval is implied.

## Architecture
Production: one fallible hayro-syntax reader → Foliark source-aware semantic model → transformations/layout/sanitization → owned pdf-writer serialization of a fresh reachable graph. Rust CPU renderer shares the semantic scene; Rust text layout produces one authoritative LayoutPlan consumed by preview and export. Do not add a second mandatory parser or use renderer outlines as the editable text model. Thin TypeScript ESM facade supervises single-threaded module worker and renders RGBA through ordinary canvas. HDDN owns viewer/editor UI, review, detection and optional external OCR orchestration. No Rust wrapper around C/C++ PDF engines qualifies.

Candidate libraries must pass NEW-4's locked WASM/build/license gate, then NEW-26's searchable native round-trip gate before broad expansion. Their existence does not prove compatible integration. If a candidate fails, document the smallest replacement/patch with the same contracts rather than force a bad architecture.

## Sequencing and exit gates
| Stage | Work | Required proof |
|---|---|---|
| Baseline | NEW-1, NEW-3 | Accepted HDDN source and reusable receipts; explicit API/support/privacy contracts. |
| Private foundation | NEW-4–NEW-13; E09 begins early | Locked WASM cohort, provenance, adversarial corpus, independent checker negative controls, fallible bounded reader. |
| Native foundation | E02 + early E04/E09 | Source model, positioned Unicode, serializer and renderer; NEW-26 independently proves searchable fresh round trip. |
| Runtime and interaction | E03/E04 | Validated protocol, revisions, cancellation/recovery, disposal, precise selections/search. |
| Privacy and editing | E05/E06/E07 | Destructive text/image/vector removal, strict retained-resource cleanup, fixed-budget Rust paragraph edits and history rules. |
| Explicit raster | E08 | Separate raster eligibility; source secrets absent, lost search/features declared; no dependency on native vector-removal success. |
| Cumulative acceptance | NEW-59, E10 | Independent security/fidelity checks, actual packed package, static/CSP/offline, comparable performance, real mobile devices. |
| HDDN migration | E11 | Current adapter/UI/OCR/save/cancel integration; remove all shipped Python/MuPDF paths; NEW-76 proves full candidate and rollback. |
| Release | E12 | Private usable candidate first; rights/license/contribution/publication decisions separate. Public release and production switch require owner-recorded go/no-go. |

Epic display order is thematic; dependency edges are authoritative. E09 starts alongside foundation, not after implementation. Independent work can run concurrently after inputs are ready; shared modules/lockfiles have one integration owner.

## Technical cohort to validate
Candidate pins supplied by research: Rust 1.99.1 / edition 2024 / wasm32-unknown-unknown; Hayro family 0.8; pdf-writer 0.15; Parley 0.12; subsetter 0.2.6; kurbo 0.13.1; wasm-bindgen 0.2.129 with matching CLI; serde-wasm-bindgen 0.6.5; ts-rs 12.0.1; TypeScript 7.0.2. These are unproved inputs, not a promise that every version exists or compiles together. Verify primary registry/upstream metadata and exact features, lock compatible font/shaping dependencies, record all actual pins. Disable system font discovery and unneeded native/thread/network features. Use Parley's current compatible HarfRust path; do not automatically choose archived RustyBuzz. Keep explicit registered/self-hosted fonts.

Primary references for implementation verification:
- https://github.com/LaurenzV/hayro
- https://docs.rs/pdf-writer/0.15.0/pdf_writer/
- https://docs.rs/parley/0.12.0/parley/
- https://rustwasm.github.io/docs/wasm-bindgen/

## Repository ownership map
- crates/foliark-model: source IDs, geometry, semantic pages, stable public DTOs.
- crates/foliark-engine: reader, interpreter, render, text/search, document transactions, redaction, sanitizer, export.
- crates/foliark-text: explicit font registry and authoritative paragraph shaping/layout.
- crates/foliark-wasm: bounded operation stepping and WASM handle facade.
- packages/sdk: validated worker protocol, supervisor, public ESM/TypeScript facade and lifecycle.
- tests/fixtures, tests/oracles, tests/integration, fuzz: rights-audited corpus and independent verification.
- examples/vanilla, tests/package-consumers: installed-artifact consumers; no framework UI library.
- tools/bench, docs: reproducible measurements, contracts, support and evidence.
File names are planned ownership boundaries. Preserve a compact implementation; move boundaries through a recorded ADR only when evidence warrants it.

## Release acceptance
No silent skips, silent raster conversion, guessed search geometry, hidden original payload recovery, stale revision acceptance or false file-save claims. Every requested operation completes or returns structured no-output failure. Supported output preserves unaffected visible content and declared searchable text. All losses/modes/effective protection regions are explicit. Real iPhone + iPad Safari and Android Chrome evidence is required before corresponding support claims. No blanket “faster/lighter” claim before same-input/quality/device measurements. Optional accelerations cannot become basic-path prerequisites.

## Decisions still belonging to owner
NEW-2: rights holder and early contribution policy. NEW-22: physical device access and measured numerical/fidelity budgets. NEW-73: commercialization/license/contribution/public identity after evidence. NEW-77: production switch. NEW-78: public publication. Private development and installation do not depend on premature public-license approval.

Read this roadmap once; then use only task-owned contracts and compact predecessor receipts. Detailed task index and implementation contracts are attached separately.