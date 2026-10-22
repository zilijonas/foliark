# Independent cumulative security and fidelity acceptance (NEW-59)

Reviewer role: adversarial, design from spec 0/5/7/8, judge with oracles that never import foliark (objects.py, fidelity.py, PyMuPDF, pypdf, pikepdf).
Candidate: the packed `@foliark/sdk` tarball (`tests/package-consumers/pack.sh`, sha256 0f8c3d21...) installed into a temp project with `npm install`, real wasm,
`@foliark/sdk/inline` in node. Not run in a browser. Runner: `tests/integration/acceptance/run.sh`, exit 0.

## Matrix
80 cases = 39 manifest fixtures + 41 adversarial (`generators/adversarial.py`, hand-written PDF syntax, deterministic) x 6 scenarios = 480 results, 0 unexplained skips:
S sanitize-only native, R native + region redactions (declared + engine-located secrets), T native + text-range redactions, r raster (acknowledged) + regions,
s raster sanitize, E edit retyping the secret then text-redacting it. Per case/scenario input/output sha256 prefixes, mode and result: `tests/integration/acceptance/RESULTS.tsv`.
Per output: objects.py `--forbid` (raw bytes, every decoded stream, strings, both extractors, UTF-16, orphans, revisions, features, image/vector regions), extra hex/octal-escaped
scan of bytes and decoded streams, fidelity.py (pixels and words outside declared change; `--raster` for raster), PyMuPDF text clipped to each redacted region, input control (the
oracle must see each secret in the input), `occurrences` for the shared Form.

| bucket | native | raster | total |
|---|---|---|---|
| SAFE (bytes exported, every oracle passes) | 242 | 143 | 385 |
| REFUSED (typed code named by the case, 0 bytes) | 41 | 17 | 58 |
| EXPLAINED n/a (reason recorded: no text secret, hidden data not in extracted text, no region) | 34 | 0 | 34 |
| UNRESOLVED BLOCKER (filed, listed in `known_blockers.json`) | 3 | 0 | 3 |

Manifest: 31 fixtures supported, 8 refused (REPAIRED_PDF x2, LIMIT_EXCEEDED x4, DECODE_FAILED, MALFORMED_PDF), as in strict-native.md; raster of forms-deep-40 and some refused
inputs exports (spec 7 is independent of native limits) and is judged like any raster. Raster RENDER_INCOMPLETE x3 (annotation-comment pinned by NEW-56).

## Defects found
| id | what | status |
|---|---|---|
| D1 | `BDC`/`DP` inline property dictionaries kept every key except ActualText/Alt/E: `SECRET_BDC_INLINE`, `SECRET_ARTIFACT_PROP`, `SECRET_DP_PROP` survived sanitize, region and text exports (case marked-props) | FIXED `writer/content.rs`: keep MCID/Type/Subtype/BBox/Attached with no string/dict value; ADR 0006; unit test, `sanitize_semantics` expectation, case |
| D2 | unknown operators (legal in `BX..EX`) kept with operands: `SECRET_BX_STRING`, `SECRET_BX_DICT` survived (case content-noise) | FIXED same change: instructions whose operator is not in PDF 32000 table 51 are dropped; same tests |
| D3 | unused Form/Image XObject (and unused fonts) stay reachable from page Resources with their text, in every native mode (case unused-xobject: `SECRET_UNUSED_FORM`; unused-font keeps BaseFont/ToUnicode names) | UNRESOLVED BLOCKER NEW-80 (P1): needs per-page/Form use analysis plus an ADR; too large for a targeted fix here. Not high/critical: invisible to page text extraction, visible to a stream scan |
Spec drift, not privacy defects: the engine evaluates `/VE` (And/Not) correctly instead of refusing `OPTIONAL_CONTENT_UNSUPPORTED` (ocg-ve visible kept, ocg-ve-hidden removed; MuPDF itself ignores /VE, so
fidelity pixels are skipped for that case and the secret must be absent); redaction over a soft-mask group is refused `INVALID_ARGUMENT feature=redactionSoftMaskGroup`, not supported as spec 5.3 says
(safe, no bytes); `EDIT_OVERLAP` never produced (NEW-53). Untouched Type3 CharProcs (`SECRET_T3_UNUSED_PROC`) survive sanitize-only by spec 5.6 and are dropped once the font is touched (R, T pass).
Harness bugs I fixed on the way (not engine): pre-NEW-47 oracle expectations for raster edge pixels (inset 0.26 pt), gray inline image colour, ActualText word glue, pypdf `'`/`"` placement.

## Correct refusals (typed, no bytes)
pattern-text-cell and pattern-image-cell: R `REDACT_PATTERN_UNSUPPORTED` (raster of the same pages burns pixels, SAFE); smask-group-text R `INVALID_ARGUMENT`; unused-font E `EDIT_NO_FIT`;
inherited-rot270 E `EDIT_ROTATED_TEXT`; 8 refused manifest fixtures x all native scenarios.

## Adversarial cases (all 41; S/R/T native, r/s raster, E edit; + safe, x refused, ~ n/a explained, ! blocker)
| case | input sha12 | R output sha12 | results |
|---|---|---|---|
| tj-kerning | e36d6371cac0 | dcadaa275297 | S+ R+ T+ r+ s+ E+ |
| split-ops | c506d7c62498 | cc867622281e | S+ R+ T+ r+ s+ E+ |
| hex-escaped | 663f6c048637 | 681442e6ccb7 | S+ R+ T+ r+ s+ E+ |
| quote-ops | 22eb82b3fe7c | 227906899a77 | S+ R+ T+ r+ s+ E+ |
| render-modes | 20c8d911a3ac | 5483f1909e01 | S+ R+ T+ r+ s+ E+ |
| spacing-neighbors | 7fa0c8a3067d | e175565a0d2d | S+ R+ T+ r+ s+ E+ |
| overprint | 5d6482d7dcb5 | 4c4bfd32bde0 | S+ R+ T+ r+ s+ E+ |
| white-and-covered | f9552c1568e3 | 4fa41cfe99a9 | S+ R+ T+ r+ s+ E+ |
| clipped-text | 5f3c156195b1 | 65e2efc827a5 | S+ R+ T+ r+ s+ E+ |
| invisible-tr3 | 8962408a653f | b20ccf8364a2 | S+ R+ T+ r+ s+ E+ |
| split-streams | 86c642933cf9 | 93e98faecaef | S+ R+ T+ r+ s+ E+ |
| ligature | 3ae997047617 | a39bad3c2e97 | S+ R+ T+ r+ s+ E+ |
| form-both-places | 8b7906cdd39c | 9404cc027eb5 | S+ R+ T~ r+ s+ E+ |
| annot-as-off | 1178399c4e75 | c9c0e38e94b9 | S+ R+ T+ r+ s+ E+ |
| annot-as-on | 8d4679e71091 | 270d63c4d91c | S+ R+ T+ r+ s+ E+ |
| annot-popup | 5884ca3951a2 | e0615e4fc002 | S+ R+ T+ r+ s+ E+ |
| smask-group-text | 15b2ded23d0a | - | S+ Rx T~ r+ s+ E~ |
| ocg-ve | 8b3025da594e | a2efa32571b7 | S+ R+ T+ r+ s+ E+ |
| ocg-ve-hidden | ace2476dfe0f | c1e379be15ad | S+ R+ T+ r+ s+ E+ |
| ocg-off-in-form | 746e2a7352ec | 2501dfcd0127 | S+ R+ T~ r+ s+ E~ |
| image-smask-only | 17b9b5e74f53 | ccb42e7e1ba7 | S+ R+ T~ r+ s+ E~ |
| image-inline | 5999c527f9fb | 90898f369f7c | S+ R+ T~ r+ s+ E~ |
| pattern-text-cell | d95b1a4ad319 | - | S+ Rx T~ r+ s+ E~ |
| pattern-image-cell | e4430aacd7da | - | S+ Rx T~ r+ s+ E~ |
| type3-glyphs | c6597c37e076 | fe6957146044 | S+ R+ T+ r+ s+ E+ |
| unused-font | 231f647cfa73 | 695fc71fdb72 | S+ R+ T+ r+ s+ Ex |
| unused-xobject | e2520faa04ae | 4963cf36180e | S! R! T! r+ s+ E+ |
| marked-props | 28f105ca9332 | a894cfa208bc | S+ R+ T+ r+ s+ E+ |
| actualtext-nested | ac9f03f73b1f | 7607422c83c8 | S+ R+ T~ r+ s+ E~ |
| content-noise | 64e22764be2d | 871c48e87aa8 | S+ R+ T+ r+ s+ E+ |
| content-chain | 3370081c3a7d | cbc2d3b31426 | S+ R+ T+ r+ s+ E+ |
| annot-hidden | 98c9f526f2d3 | f95f37ac4730 | S+ R+ T+ r+ s+ E+ |
| rotated-text-30 | 01e99c6a1034 | f166675c927e | S+ R+ T+ r+ s+ E+ |
| hidden-everywhere | 5e06218ccfc4 | - | S+ R~ T~ r+ s+ E~ |
| incremental-info | 3df6cf27e50c | - | S+ R~ T~ r+ s+ E~ |
| orphan-trailing | cc600e665cbb | 207a5453ef87 | S+ R+ T+ r+ s+ E+ |
| objstm-orphan | 6628f1ed5f1e | efd8a2484600 | S+ R+ T+ r+ s+ E+ |
| cropbox-outside | 1879a3981598 | ba0e0496e144 | S+ R+ T+ r+ s+ E+ |
| inherited-rot270 | 979d2e344d95 | beddbf1254c3 | S+ R+ T+ r+ s+ Ex |
| shared-stream-pages | 0598db806bd2 | 28c8ebc3458c | S+ R+ T~ r+ s+ E+ |
| multipage-12 | 18749da1b015 | 85870dca3a18 | S+ R+ T+ r+ s+ E+ |

Covered placements: kerning-split TJ, split Tj, split content streams, hex/octal strings, `'`/`"`, render modes 1/2/3/7, Tc/Tw/Tz/Ts neighbours, overprint copies (0.3/0.5 pt offsets), white and covered ink,
clipped-out text, invisible Tr 3, fi ligature code, rotated text, one Form drawn inside and outside the region (outside copy must keep it), two pages sharing one content stream (page 2 only),
annotation /AS Off/On states, Hidden/NoView annotations, Popup /Contents, soft-mask group, /VE both ways, OCG off in a Form, SMask-only image, inline image, tiling pattern text/image cell, Type3, unused font,
unused Form, marked-content properties, comments/BX/unknown operators, ASCII85+Flate chain, Info/XMP (catalog, page)/PieceInfo/private keys/JS/OpenAction/AA/embedded file/AF/outline/OutputIntent/StructTree
Alt/ActualText/E/thumbnail/Names/Dests/PageLabels/ViewerPreferences, prior incremental revision, orphan object plus trailing garbage after %%EOF, 1.5 object stream with unreachable compressed object, CropBox
with text outside, inherited Rotate 270, 12 pages.

## Negative controls (all caught)
109 leaky variants judged by the same function: 52 sanitize-only bytes presented as redacted, 52 fake overlays (black boxes appended over live text), and on one good output: secret in /Info, hex string in an
extra stream, secret in a private catalog key, trailing orphan object, redrawn text. 0 missed; a variant counts only if it really still shows the secret. `tests/oracles/controls.py` still passes.

## Cancellation
27 attempts on a 12-page redaction (apply, prepare native, prepare raster; pre-aborted, sync, and 0-32 event-loop hops, ops queued ahead). Each final export equals a clean run byte for byte
(sha256 a759ec44... native); an aborted apply is atomic (revision 0 => re-apply equals the clean run). Only 3 attempts (the pre-aborted ones) ended CANCELLED.

## Not proven
- Mid-flight abort through the packed SDK: the in-process worker finishes before an abort lands, so abort-while-running is covered only by NEW-58 (internal stepBudget), not here.
- JPX/JBIG2/CCITT payloads, CFF/Type1 programs, ICC interior, real browser (Playwright), second renderer (MuPDF only, blind to /VE), CID fonts without ToUnicode, secrets that are not ASCII markers.
- Encrypted raster fidelity (oracle takes one password): 5 fixtures judged by objects.py only. Edit scenario is n/a where the secret is not in extracted text (34 explained rows).
- Raster pixels are checked with an inset of 0.26 pt (squares only touching a region edge are not required black). 41 synthetic fixtures do not certify arbitrary real-world files.

## Reproduce
```sh
~/ai/bin/memory-queue run --owner <id> --label 'foliark acceptance' --need-gb 4 --restartable --wait 1800 -- tests/integration/acceptance/run.sh   # SKIP_BUILD=1 reuses dist/wasm
tests/oracles/.venv/bin/python tests/integration/acceptance/generators/adversarial.py target/acceptance/adv   # fixtures only
cargo test -p foliark-engine --lib writer::content ; cargo test --workspace   # 359 passed; sdk vitest 78 passed
```
