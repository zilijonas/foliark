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
| P10 | interpret `device.rs`, `font/mod.rs`, `interpret/{mod,text,state}.rs`, `context.rs`, `pattern.rs`, `font/type3.rs` | ~60 | `PositionedGlyph::advance()` (advance in em with Tc/Tw) and `code_index()` (index of the code within the show operator, counting undrawn codes); `GlyphRun::render_mode()` (Tr 0-7); mode 7 (clip only) is reported as `DrawMode::Invisible` like mode 3 (the renderer draws nothing for it, as before); `Device::unknown_operator(name)` for an operator the interpreter skips | yes | done (NEW-14) |
| P11 | interpret `interpret/mod.rs`, `context.rs`, `font/{mod,standard_font,type1,true_type}.rs` | ~25 | `InterpreterWarning::FontSubstituted(Option<ObjRef>)`: a simple font (Type1, TrueType) that is not embedded and is not exactly one of the 14 standard faces is drawn with a standard face; reported once per font and render cache when the font is first resolved, only with the new `InterpreterSettings::report_font_substitution` (default false, so existing consumers see no new warnings). CID fonts already ask `font_resolver` with `FontQuery::Fallback` | yes | done (NEW-25) |
| P12 | syntax `object/stream.rs` | ~4 | `Stream::raw_data_strict()`: decrypted but still filtered bytes, decryption failure is `Err` (the writer copies allowlisted filter chains such as DCT/JPX raw) | yes | done (NEW-24) |
| P13 | syntax `object/array.rs`, `object/dict.rs` | ~8 | Fuzz fixes (NEW-57): `ArrayIter::next` and `Dict::entries` unwrapped a re-read of a value that was validated at first parse; a stream-like dict inside an array (`[<<>>stream]`) fails the re-read and panicked. Iteration now ends / the entry is skipped. Only a panic path changes; valid input behaves as before | yes | done (NEW-57) |
| P14 | interpret `interpret/mod.rs`, `x_object/image.rs` | ~15 | `InterpreterSettings::max_image_pixels` (default `u64::MAX`): `ImageXObject::draw` skips an image or inline image over it and reports `InterpreterWarning::ImageTooLarge(w, h)`, before any decode. Found by fuzzing (NEW-57): a 100000x10000 image allocated 30 GB in the renderer | yes | done (NEW-57) |
| P15 | interpret `interpret/mod.rs`, `context.rs` | ~20 | `InterpreterSettings::should_stop: Option<StopFn>` (`Arc<dyn Fn() -> bool + Send + Sync>`, default `None`), asked before every operator in every interpretation loop (page, Form, pattern, Type3, annotation appearance); once it returns `true` the remaining operators are skipped. The output is then incomplete and the caller must discard it. A Device cannot stop the interpreter, so this is how cancellation and the work budget end a page mid-stream. Settings are cloned into nested contexts and into `hayro::render_into`, so the renderer honours it too | yes | done (NEW-29) |
