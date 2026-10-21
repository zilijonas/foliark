# Vendored hayro fork

`vendor/hayro-syntax` and `vendor/hayro-interpret` are wired through `[patch.crates-io]` in the root `Cargo.toml`. The renderer `hayro` 0.8.0 stays on crates.io and picks up the patched interpret.

## Baseline

Unmodified upstream 0.8.0, copied from the crates.io source with only `.cargo-ok`, `.cargo_vcs_info.json`, `Cargo.lock` and `Cargo.toml.orig` removed.

| Crate | Version | .crate sha256 |
|---|---|---|
| hayro-syntax | 0.8.0 | `6cdc818d8d265dcb42081d488f7599df4f73fb79e9d347ecedcd3c23809d2026` |
| hayro-interpret | 0.8.0 | `ac9df074ff163a63864cec9b0bf640ccbff8abf4e44481df7369ce0ac172ffbe` |

## Rules

- Additive only: new methods, fields behind new types, new trait methods with default bodies. Never change existing behaviour of upstream API.
- Every change is upstreamable: self-contained, no Foliark types, documented like the surrounding code.
- One row per change below, in the same commit as the change. Planned set: spec `docs/spec/v1.md` section 13.1.

## Patches

LOC = added/changed lines (approximate, rustfmt'd). Spec: `docs/spec/v1.md` 13.1.

| ID | Files | LOC | Why | Upstreamable | Status |
|---|---|---|---|---|---|
| P1 | syntax `content/mod.rs`, `page.rs` | ~85 | `UntypedIter/TypedIter::next_with_span` + `offset` give each operator's byte range; `Page::content_parts()` gives the per-content-stream object and range inside the merged `page_stream` (redaction rewrites operators in one specific stream) | yes | done (NEW-10) |
| P2 | interpret | - | `Device::begin_operator/begin_form/end_form` identity stack | yes | planned |
| P3 | interpret | - | glyph raw char code, font ObjRef getters | yes | planned |
| P4 | interpret | - | `InterpreterSettings::ocg_override` | yes | planned |
| P5 | syntax `filter/lzw_flate.rs`, `filter/mod.rs`, `object/stream.rs`; interpret half in NEW-12 | ~115 (syntax) | `Stream::decoded_strict()`: decrypt failure, corrupt/garbage/truncated Flate, bad back-reference are `Err` instead of empty or partial data. `decoded()` and every upstream caller keep the lenient behaviour | yes | syntax done (NEW-10) |
| P6 | syntax `pdf.rs`, `xref.rs` | ~14 | `Pdf::was_repaired()` / `XRef::was_repaired()` expose the existing repair flag (repair is lazy, so ask again after reading) | yes | done (NEW-10) |
