# Native round-trip evidence (NEW-26)

Gate: `crates/foliark-engine/tests/integration_roundtrip.rs` (Cargo test target, 1 test) plus
`tests/integration/native_roundtrip/oracles.py` (independent checks). Per-feature behaviour stays in
`native_graphics_roundtrip.rs` / `native_text_roundtrip.rs`; this gate proves the whole chain on all 35 manifest fixtures.

## Chain per supported fixture
strict open -> semantic model (`page_model`) -> Foliark words -> pass-1 `Writer` output (annotation / widget flattening
and optional-content / off-page resolution, the first pass of native export; written twice, bytes must be identical) ->
strict reopen -> Foliark words again (text, order, boxes within 0.5 pt; flattened appearances may add words) -> Foliark
render of input and output at 1 px/pt (0 channel values differ by more than 2/255) -> oracles on the output:
`objects.py` (single revision, no orphans, no /Info /XMP /EmbeddedFiles /JS /OpenAction /AA /AcroForm /OCProperties /Encrypt),
`fidelity.py` (PyMuPDF render + words vs input), PyMuPDF words and pypdf text vs the input (`--flatten`: the flattened
appearance may extend a line; `--layers`: pypdf is layer-blind, so removed hidden text is the only allowed difference).
Encrypted inputs are compared against a pikepdf-decrypted copy; outputs are never encrypted.

## Result: 35 fixtures = 25 supported (1 with a disclosed removal) + 1 stripped by design + 8 refused (typed), 0 silent
The test prints `fixtures 35 supported 25 (disclosed removal 1) refused 8`. Columns: bytes in / out, sha256[:12] in / out,
pages words items (or refusal code and stage).

| fixture | class | bytes | sha256 | detail |
|---|---|---|---|---|
| metadata-xmp | supported | 1218 / 707 | 03e27e03b609 / b7eb8ece57c1 | 1p 2w 43i |
| annotation-comment | supported (disclosed removal) | 909 / 713 | d2619514f210 / 471b0b085359 | ANNOTATIONS_REMOVED disclosed (no /AP: nothing drawable) |
| attachment | supported | 1137 / 705 | 8e251be09ec1 / 430ecb9dd195 | 1p 2w 39i |
| actions-js | supported | 1254 / 705 | 8ece2e7c3625 / 34d6e87e88d8 | 1p 2w 39i |
| acroform | supported | 1338 / 1282 | 4e80f8217d80 / 69b78284418c | 1p 3w 53i |
| optional-content-layer | supported | 1002 / 762 | e200e108f44d / 01928a70f81e | 1p 2w 80i |
| hidden-ocr-text | supported | 800 / 734 | 3b7c9e27e5fd / b3e04c66f133 | 1p 3w 69i |
| thumbnail | supported | 60916 / 704 | 3f342c70d2a8 / 0827e041d251 | 1p 2w 37i |
| outline-bookmark | supported | 1015 / 711 | e6182519db1d / 4d71d9001d8a | 1p 2w 51i |
| incremental-revision | supported | 1231 / 715 | c889e72739c0 / f5c28ef60a96 | 1p 2w 46i |
| actualtext | stripped (by design) | - | - | inline ActualText removed (spec 8.2); judged in sanitize_document.rs |
| shared-image | supported | 61355 / 3517 | 4d627708bc9d / 0123c88ac25e | 2p 3w 76i |
| shared-vector | supported | 1666 / 1532 | ac662396cbc3 / a5207f7cea4a | 2p 3w 113i |
| rotation-crop | supported | 1915 / 1947 | 37e25c655f90 / 8eda26d332f6 | 4p 8w 180i |
| unicode-glyph-widths | supported | 41049 / 22658 | c6ab1d3a8887 / 060eeaea7e62 | 1p 6w 58i |
| encrypted | supported | 1096 / 704 | c04d1d9dbfce / ba7d6f1a205f | 1p 2w 37i |
| malformed-truncated | refused | 536 / - | f8213d2aeb4e / - | RepairedPdf at open |
| malformed-xref | refused | 739 / - | 8f8191b5b2d7 / - | RepairedPdf at open |
| oversized-image | refused | 978 / - | 75c54c8e4ed6 / - | LimitExceeded at model |
| compressed-stream | refused | 9023 / - | ba6dd7b24a1e / - | LimitExceeded at model |
| corrupt-flate | refused | 649 / - | 2b22528e19c5 / - | DecodeFailed at model |
| forms-cyclic | refused | 1135 / - | ddc81e58dad7 / - | MalformedPdf at model |
| forms-deep-40 | refused | 9097 / - | a3e675f84b58 / - | LimitExceeded at model |
| form-reuse-matrices | supported | 941 / 1072 | 426c4ba5e820 / 98c688c3324b | 1p 4w 76i |
| many-pages-1001 | refused | 156457 / - | 2cd12540fb8a / - | LimitExceeded at open |
| ocg-hidden-text | supported | 966 / 742 | b0056a28a9ed / e19ada043c94 | 1p 2w 52i |
| ocr-over-image | supported | 951 / 1035 | 851a2cd64d1b / 37ee5dd75f15 | 1p 2w 36i |
| leaks-metadata-attachment-js | supported | 1736 / 687 | a19ab47d6c2a / 0bb6f6cfad3a | 1p 1w 19i |
| incremental-prior-text | supported | 797 / 685 | 54734f09d875 / 7edcb8d9ddd5 | 1p 1w 17i |
| acroform-values | supported | 1163 / 1262 | ffb8b8e74993 / 5ff16bb170df | 1p 2w 34i |
| annot-appearance | supported | 1257 / 1263 | 45333ccdfa66 / b1b98e330858 | 1p 2w 35i |
| encrypted-rc4-40 | supported | 909 / 710 | 89b74529499d / 6190deca6818 | 1p 2w 42i |
| encrypted-rc4-128 | supported | 910 / 710 | 3da93f7ea5b0 / 6190deca6818 | 1p 2w 42i |
| encrypted-aes-128 | supported | 1009 / 710 | 353f8ec9d1e1 / 6190deca6818 | 1p 2w 42i |
| encrypted-aes-256 | supported | 1251 / 710 | 8d90fb7374c3 / 6190deca6818 | 1p 2w 42i |
The four encrypted outputs share one hash: same plaintext page, deterministic writer.

## Classes and what is NOT proven
- supported: native text (base-14, embedded font, ligature, non-BMP), vector, images, Forms, rotation/CropBox, multi-page,
  shared resources, invisible OCR text, incremental revisions (prior revision gone), RC4-40/128, AES-128/256 inputs,
  annotations and widgets with an appearance (flattened onto the page: acroform, acroform-values, annot-appearance), and
  optional content (optional-content-layer, ocg-hidden-text: hidden layers removed, default configuration kept visible).
- supported with a disclosed removal (annotation-comment): the annotation has no /AP, so nothing is drawable. It is removed
  and export reports `ANNOTATIONS_REMOVED`; the gate asserts the stage counts it, the output has no /Annots, objects.py
  passes, and the page content round-trips. The oracles must still see that icon (MuPDF invents one) go.
- stripped by design (actualtext): inline ActualText is removed, so extractors read the drawn glyphs.
- refused (typed, asserted per code): hostile inputs stopped by the reader or limits before any writer (REPAIRED_PDF for
  repaired xref or truncation, LIMIT_EXCEEDED for image, decode-ratio, 40-deep Form and page-count bombs, DECODE_FAILED,
  MALFORMED_PDF for the cyclic Form). No fixture here is refused by a native export stage.
- Native export stages that still refuse by name, outside this gate (no manifest fixture hits them): optional content a
  layer expression cannot decide (OPTIONAL_CONTENT_UNSUPPORTED, `flattenOptionalContent`), widgets with a value but no or a
  stale appearance (FORM_APPEARANCE_MISSING), XFA (XFA_UNSUPPORTED), `invisibleText: remove` (`invisibleTextRemove`), and
  redaction over tiling-pattern paint or soft-mask groups. Raster mode is independent of all of them.
- capability-unknown, no fixture in this gate: CFF/Type1 font program retention, inline images in the image oracle, Type3
  fonts, shadings/patterns in the vector oracle. Redaction and edits are gated in their own test targets.
- Limits: this gate runs pass 1 of the pipeline on the writer, not the whole export; one Foliark renderer on both sides
  (render parity shows the writer is faithful, not that the renderer is right; PyMuPDF `fidelity.py` is the independent
  appearance check); pypdf comparison is whitespace-insensitive text; the fixtures are small and synthetic.

## Reproduce
```sh
python3 -m venv tests/oracles/.venv && tests/oracles/.venv/bin/pip install --only-binary=:all: -r tests/oracles/requirements.txt
~/ai/bin/memory-queue run --owner <id> --label 'foliark roundtrip' --need-gb 4 --restartable --wait 1800 -- \
  cargo test -p foliark-engine --test integration_roundtrip -- --nocapture
```
Negative control: `oracles.py shared-vector.pdf rotation-crop.pdf` reports PAGES, page count and pypdf text differences.
