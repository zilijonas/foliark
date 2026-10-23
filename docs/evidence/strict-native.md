# Strict native export safety proof (NEW-47)

Gate: `crates/foliark-engine/tests/strict_native.rs` (1 Cargo test) + `tests/integration/strict_native/check.py` (independent checks).
Inputs: all 39 entries of `tests/fixtures/manifest.json` = the 35 manifest inputs + 4 new controls (generator `generate.py`,
`verify.py --regen` clean): `mixed-regions` (text, image, vector, Form secrets), `icc-tail-secret`, `jpeg-payload-secret`
(COM, APP2, bytes after EOI, image under a region), `font-payload-secret` (marker in the TrueType name table). No skips: every entry is a row.

## Per fixture, through the public engine
- **R** native export with a redaction over every seeded secret (Foliark text offsets for each `mustRemove` string, plus a region per
  declared sensitive region / HDDN target box), default policy. **S** sanitize-only export (no changes).
- Checks, all independent of Foliark (pikepdf, pypdf, PyMuPDF via tests/oracles):
  input control (objects.py finds each secret in the input, else a clean output proves nothing); objects.py on R (secrets absent from bytes,
  every decoded stream, strings, both text extractors; reachability, forbidden keys, single revision, dangling refs; image regions
  black; vector regions only our fill); fidelity.py R vs input (pixels and words outside the redactions and the secrets own words;
  public words extractable by PyMuPDF and pypdf); on S the same with no declared change, and every secret PyMuPDF cannot read from the
  input page must be gone. Export report must disclose each feature the fixture has (metadata, attachments, actions, outlines, thumbnails,
  structure, layers, widgets, annotations, invisible text, encryption).
- Negative controls in the same run: R replaced by the S bytes for mixed-regions, ocr-over-image, jpeg-payload-secret must be caught
  (IMAGE_REGION, VECTOR_REGION, STREAM, TEXT_FITZ as applicable). Refused: both runs must fail with the exact code and the engine holds 0 plan bytes.

## NEW-100 addendum (2026-10-23): 42 cases = 34 supported + 8 refused, 0 skipped
Two font-shape fixtures from real documents joined the manifest (`cjk-predefined-cmaps` had joined in NEW-88):
`quartz-type0-empty-tounicode` (iOS Quartz Type0 Identity-H, ToUnicode with no mapping) and `word-winansi-type0-twins` (Word's
WinAnsi TrueType plus Identity-H twin). Both extractors read the Quartz glyphs as junk, so objects.py gained TEXT_GLYPH (glyphs named
by their font program's own cmap; leaky and safe controls in `controls.py`) and check.py judges outputs against `objects.named_copy`
of the input (ToUnicode filled from that cmap, nothing drawn changes); the input control still runs on the unmodified input.

| fixture | outcome | checks | hashes |
|---|---|---|---|
| quartz-type0-empty-tounicode | supported | R+S | d6985a96803a / cc269192cc7d / 65cac66df2b4 |
| word-winansi-type0-twins | supported | R+S | 3e007021c9c1 / 90386d293391 / 5e7ccd870465 |

## Result: 39 cases = 31 supported + 8 refused (typed, no bytes), 0 skipped
Refused: REPAIRED_PDF x2 (malformed-truncated, malformed-xref), LIMIT_EXCEEDED x4 (oversized-image, compressed-stream, forms-deep-40,
many-pages-1001), DECODE_FAILED x1 (corrupt-flate), MALFORMED_PDF x1 (forms-cyclic). Table: sha256[:12] input / R / S.

| fixture | outcome | checks | hashes |
|---|---|---|---|
| metadata-xmp | supported | R+S | 03e27e03b609 / af6429099a16 / b7eb8ece57c1 |
| annotation-comment | supported | R+S | d2619514f210 / 8a53db6e32be / 471b0b085359 |
| attachment | supported | R+S | 8e251be09ec1 / e71b5a3c5cf0 / 430ecb9dd195 |
| actions-js | supported | R+S | 8ece2e7c3625 / 59fcc474686a / 34d6e87e88d8 |
| acroform | supported | R+S | 4e80f8217d80 / 1a4065f71cb7 / 86b84b654939 |
| optional-content-layer | supported | R+S | e200e108f44d / c2038dc0ea88 / 01928a70f81e |
| hidden-ocr-text | supported | R+S | 3b7c9e27e5fd / dda548110fc8 / b3e04c66f133 |
| thumbnail | supported | R+S | 3f342c70d2a8 / c324f4f72def / 0827e041d251 |
| outline-bookmark | supported | R+S | e6182519db1d / f33c1e0b5bc5 / 4d71d9001d8a |
| actualtext | supported | R+S | 7394dd868b36 / 139958a46334 / 0016832d18ce |
| incremental-revision | supported | R+S | c889e72739c0 / f5c28ef60a96 / f5c28ef60a96 |
| shared-image | supported | R+S | 4d627708bc9d / f5bada24cec9 / 0123c88ac25e |
| shared-vector | supported | R+S | ac662396cbc3 / c45539e6422c / a5207f7cea4a |
| rotation-crop | supported | R+S | 37e25c655f90 / 5846d01ed6a6 / 8eda26d332f6 |
| unicode-glyph-widths | supported | R+S | c6ab1d3a8887 / 4c4db0ea5015 / 4dd3821a55c5 |
| encrypted | supported | R+S | c04d1d9dbfce / 4ee8cfb552be / ba7d6f1a205f |
| malformed-truncated | refused RepairedPdf | none (both runs) | f8213d2aeb4e |
| malformed-xref | refused RepairedPdf | none (both runs) | 8f8191b5b2d7 |
| oversized-image | refused LimitExceeded | none (both runs) | 75c54c8e4ed6 |
| compressed-stream | refused LimitExceeded | none (both runs) | ba6dd7b24a1e |
| corrupt-flate | refused DecodeFailed | none (both runs) | 2b22528e19c5 |
| forms-cyclic | refused MalformedPdf | none (both runs) | ddc81e58dad7 |
| forms-deep-40 | refused LimitExceeded | none (both runs) | a3e675f84b58 |
| form-reuse-matrices | supported | R+S | 426c4ba5e820 / 5652715e45fb / 98c688c3324b |
| many-pages-1001 | refused LimitExceeded | none (both runs) | 2cd12540fb8a |
| ocg-hidden-text | supported | R+S | b0056a28a9ed / ca6984ca25a1 / e19ada043c94 |
| ocr-over-image | supported | R+S | 851a2cd64d1b / 54df828c883d / 37ee5dd75f15 |
| leaks-metadata-attachment-js | supported | R+S | a19ab47d6c2a / 0bb6f6cfad3a / 0bb6f6cfad3a |
| incremental-prior-text | supported | R+S | 54734f09d875 / 7edcb8d9ddd5 / 7edcb8d9ddd5 |
| acroform-values | supported | R+S | ffb8b8e74993 / 43a14d4b0c4b / 774e9e245cb0 |
| annot-appearance | supported | R+S | 45333ccdfa66 / b388a3b5f241 / aa77758f639f |
| mixed-regions | supported | R+S | 7285493d981b / acaedec01b6f / bce9c07ea7d2 |
| icc-tail-secret | supported | R+S | 9238336c2aae / d458de785913 / b41850e93c1a |
| jpeg-payload-secret | supported | R+S | b916b36e8f84 / 5422827e27db / dacb8a1a6ebd |
| font-payload-secret | supported | R+S | 0aaca1026697 / 6b6523ebb1d6 / 4dd3821a55c5 |
| encrypted-rc4-40 | supported | R+S | 89b74529499d / c52980f99f98 / 6190deca6818 |
| encrypted-rc4-128 | supported | R+S | 3da93f7ea5b0 / c52980f99f98 / 6190deca6818 |
| encrypted-aes-128 | supported | R+S | 353f8ec9d1e1 / c52980f99f98 / 6190deca6818 |
| encrypted-aes-256 | supported | R+S | 8d90fb7374c3 / c52980f99f98 / 6190deca6818 |

## Not proven
- JPX, JBIG2 and CCITT payloads (kept byte-for-byte; no control), content inside an ICC profile's declared size, CFF/Type1 programs
  (kept, listed in kept.fontsWithGlyphInventory), Type3 CharProcs drawing with other fonts, inline images in the image oracle, shadings
  and patterns in the vector oracle, SMaskInData, redaction over tiling-pattern paint or soft-mask groups (refused by name, no fixture here).
- Image erasure is a closed-set test: samples touching a region edge are erased too. fidelity.py is told to expect one source sample (4 pt)
  of change around image regions. Redacted invisible text that overlaps a public image erases those pixels (ocr-over-image).
- pypdf cannot place text inside transformed Forms or on rotated pages: its chunk finding is dropped only when every chunk it names is a
  redacted secret; ActualText fixtures skip word comparison (removed by design, spec 8.2); OCG fixtures skip pypdf chunks (layer-blind).
- One renderer (PyMuPDF/MuPDF) for pixels; small synthetic fixtures; secrets are ASCII markers.

## Bugs found
- oracle: objects.py reported a false DECODE finding for every DCT image (read_bytes on an image codec stream); fixed. No engine bug found.

## Reproduce
```sh
python3 -m venv tests/oracles/.venv && tests/oracles/.venv/bin/pip install --only-binary=:all: -r tests/oracles/requirements.txt
~/ai/bin/memory-queue run --owner <id> --label 'foliark strict_native' --need-gb 4 --restartable --wait 1800 -- \
  cargo test -p foliark-engine --test strict_native -- --nocapture
tests/fixtures/generators/generate.py && tests/oracles/.venv/bin/python tests/fixtures/verify.py --regen
```
