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
| P2 | interpret `device.rs`, `context.rs`, `interpret/mod.rs`, `x_object/form.rs`, `pattern.rs`, `font/type3.rs` | ~150 | `Device::begin_operator(stream, range)` (stream via `Context::source`, per-page-part mapping), `begin_form(obj, matrix) -> bool` / `end_form` (Form, annotation appearance, tiling pattern, Type3 procedure; returning false skips it), forwarded by the stencil/Type3 wrapper devices. Default no-ops, renderer unchanged | yes | done (NEW-12) |
| P3 | interpret `font/mod.rs`, `types.rs` | ~55 | `Glyph/OutlineGlyph/Type3Glyph::char_code()` and `font_ref()` (font dict ObjRef); `StencilImage::stream()` so stencil images have an object id like raster ones | yes | done (NEW-12) |
| P4 | interpret `ocg.rs`, `interpret/mod.rs`, `path.rs`, `text.rs`, `x_object/*` | ~30 | `InterpreterSettings::ocg_override` interprets content in OFF groups; `Device::oc_state(visible)` is called before each item, `false` for hidden content | yes | done (NEW-12) |
| P5 | syntax `filter/lzw_flate.rs`, `filter/mod.rs`, `object/stream.rs`; interpret `util.rs`, `interpret/mod.rs`, `x_object/*`, `pattern.rs`, `font/type3.rs` | ~115 syntax, ~65 interpret | `Stream::decoded_strict()`: decrypt failure, corrupt/garbage/truncated Flate, bad back-reference are `Err` instead of empty or partial data (`decoded()` and upstream callers stay lenient). Interpret half: `InterpreterSettings::strict_streams` skips an undecodable Form/pattern/Type3/appearance stream and reports `InterpreterWarning::StreamDecodeFailure(obj)`; `UnresolvedXObject(obj)` reports an XObject the parser dropped (missing, damaged or reference cycle) | yes | done (NEW-10 syntax, NEW-12 interpret) |
| P6 | syntax `pdf.rs`, `xref.rs` | ~14 | `Pdf::was_repaired()` / `XRef::was_repaired()` expose the existing repair flag (repair is lazy, so ask again after reading) | yes | done (NEW-10) |
| P7 | syntax `filter/*.rs`, `object/stream.rs` | ~45 | `Stream::decoded_capped(max)`: strict decode that returns `DecodeFailure::TooLarge` once the output exceeds `max` bytes. Flate (both the flate2 and the built-in decoder), LZW and run-length stop at `max + 1` bytes, so a decompression bomb is rejected before it is allocated; every other filter is bounded by its input and checked after the fact. `decoded()` and `decoded_strict()` are unchanged (cap = unlimited) | yes | done (NEW-13) |
| P8 | syntax `xref.rs`, `pdf.rs` | ~10 | `Pdf::is_encrypted()` / `XRef::is_encrypted()`: whether a decryptor is active, so a caller can report it without probing a stream | yes | done (NEW-11) |
| P9 | syntax `crypto/mod.rs` | ~40 | Owner-password authentication for security handler revisions 2 to 4 (RC4-40/128, AES-128), PDF 32000 algorithm 7: the owner password decrypts `/O` to the user password, which is then checked as before. Upstream only accepted the user password there (revision 5/6 already accepted both) | yes | done (NEW-11) |
