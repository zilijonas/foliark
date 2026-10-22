# Native round-trip evidence (NEW-26)

Gate: `crates/foliark-engine/tests/integration_roundtrip.rs` (Cargo test target, 1 test) plus
`tests/integration/native_roundtrip/oracles.py` (independent checks). Per-feature behaviour stays in
`native_graphics_roundtrip.rs` / `native_text_roundtrip.rs`; this gate proves the whole chain on all 35 manifest fixtures.

## Chain per supported fixture
strict open -> semantic model (`page_model`) -> Foliark words -> fresh `Writer` output (written twice, bytes must be
identical) -> strict reopen -> Foliark words again (text, order, boxes within 0.5 pt) -> Foliark render of input and
output at 1 px/pt (0 channel values differ by more than 2/255) -> oracles on the output:
`objects.py` (single revision, no orphans, no /Info /XMP /EmbeddedFiles /JS /OpenAction /AA /AcroForm /OCProperties /Encrypt),
`fidelity.py` (PyMuPDF render + words vs input), PyMuPDF words and pypdf text equal to the input (boxes within 0.5 pt).
Encrypted inputs are compared against a pikepdf-decrypted copy; outputs are never encrypted.

## Result: 35 fixtures = 21 supported + 4 flatten-pending + 10 refused (typed), 0 skipped, 0 silent
Supported totals: 26 pages, 56 Foliark words (identical in and out), 1156 semantic items, 184,402 B in, 42,461 B out.
Columns: bytes in / out, sha256[:12] in / out, pages words items (or refusal code and stage).

| fixture | class | bytes | sha256 | detail |
|---|---|---|---|---|
| metadata-xmp | supported | 1218 / 707 | 03e27e03b609 / b7eb8ece57c1 | 1p 2w 43i |
| annotation-comment | flatten-pending | 909 / 713 | d2619514f210 / 471b0b085359 | annotations dropped |
| attachment | supported | 1137 / 705 | 8e251be09ec1 / 430ecb9dd195 | 1p 2w 39i |
| actions-js | supported | 1254 / 705 | 8ece2e7c3625 / 34d6e87e88d8 | 1p 2w 39i |
| acroform | flatten-pending | 1338 / 703 | 4e80f8217d80 / c0125e9a65c4 | annotations dropped |
| optional-content-layer | refused | 1002 / - | e200e108f44d / - | OptionalContentUnsupported at write |
| hidden-ocr-text | supported | 800 / 734 | 3b7c9e27e5fd / b3e04c66f133 | 1p 3w 69i |
| thumbnail | supported | 60916 / 704 | 3f342c70d2a8 / 0827e041d251 | 1p 2w 37i |
| outline-bookmark | supported | 1015 / 711 | e6182519db1d / 4d71d9001d8a | 1p 2w 51i |
| actualtext | supported | 1246 / 803 | 7394dd868b36 / dac44c8f192b | 1p 3w 52i |
| incremental-revision | supported | 1231 / 715 | c889e72739c0 / f5c28ef60a96 | 1p 2w 46i |
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
| ocg-hidden-text | refused | 966 / - | b0056a28a9ed / - | OptionalContentUnsupported at write |
| ocr-over-image | supported | 951 / 1035 | 851a2cd64d1b / 37ee5dd75f15 | 1p 2w 36i |
| leaks-metadata-attachment-js | supported | 1736 / 687 | a19ab47d6c2a / 0bb6f6cfad3a | 1p 1w 19i |
| incremental-prior-text | supported | 797 / 685 | 54734f09d875 / 7edcb8d9ddd5 | 1p 1w 17i |
| acroform-values | flatten-pending | 1163 / 684 | ffb8b8e74993 / e23a078343de | annotations dropped |
| annot-appearance | flatten-pending | 1257 / 685 | 45333ccdfa66 / f6bc070b0732 | annotations dropped |
| encrypted-rc4-40 | supported | 909 / 710 | 89b74529499d / 6190deca6818 | 1p 2w 42i |
| encrypted-rc4-128 | supported | 910 / 710 | 3da93f7ea5b0 / 6190deca6818 | 1p 2w 42i |
| encrypted-aes-128 | supported | 1009 / 710 | 353f8ec9d1e1 / 6190deca6818 | 1p 2w 42i |
| encrypted-aes-256 | supported | 1251 / 710 | 8d90fb7374c3 / 6190deca6818 | 1p 2w 42i |
The four encrypted outputs share one hash: same plaintext page, deterministic writer.
## Classes and what is NOT proven
- supported: native text (base-14, embedded font, ligature, non-BMP), vector, images, Forms, rotation/CropBox, multi-page,
  shared resources, invisible OCR text, incremental revisions (prior revision gone), RC4-40/128, AES-128/256 inputs.
- flatten-pending (annotation-comment, acroform, acroform-values, annot-appearance): page content, words and Foliark render
  (annotations off) round-trip, objects.py passes, output has no /Annots /Widget, and the oracles must still see the loss
  (pixels or words); output Foliark words must be an in-order subset of the input words. NOT a pass: annotations and form
  fields are dropped until flattening exists (spec 8.2).
- refused: hidden optional content (OPTIONAL_CONTENT_UNSUPPORTED at write, OCG flattening pending); hostile inputs stopped
  by the reader or limits before any writer (REPAIRED_PDF for repaired xref or truncation, LIMIT_EXCEEDED for image,
  decode-ratio, 40-deep Form and page-count bombs, DECODE_FAILED, MALFORMED_PDF for the cyclic Form). Each code is asserted.
- capability-unknown / unsupported, no fixture in this gate: CFF/Type1 font program retention, inline images in the image
  oracle, Type3 fonts, shadings/patterns in the vector oracle, optional content that is visible, redaction, edits.
- Limits: one Foliark renderer on both sides (render parity shows the writer is faithful, not that the renderer is right;
  PyMuPDF `fidelity.py` is the independent appearance check); pypdf comparison is whitespace-insensitive text; 21 supported
  fixtures are small and synthetic.

## Reproduce
```sh
python3 -m venv tests/oracles/.venv && tests/oracles/.venv/bin/pip install --only-binary=:all: -r tests/oracles/requirements.txt
~/ai/bin/memory-queue run --owner <id> --label 'foliark roundtrip' --need-gb 4 --restartable --wait 1800 -- \
  cargo test -p foliark-engine --test integration_roundtrip -- --nocapture
```
The test prints `fixtures 35 supported 21 flatten-pending 4 refused 10` and one row per fixture. Negative control:
`oracles.py shared-vector.pdf rotation-crop.pdf` reports PAGES, page count and pypdf text differences.
