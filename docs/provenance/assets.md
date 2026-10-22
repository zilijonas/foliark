# Asset and code provenance (NEW-5)

Facts from the repo and the cargo registry copies. Not legal clearance.

## (a) Vendored code

| Crate | Upstream | License | Patched? | Source |
|---|---|---|---|---|
| hayro-syntax | 0.8.0 (crates.io) | Apache-2.0 OR MIT | no (src and Cargo.toml byte-identical to upstream) | `vendor/hayro-syntax` |
| hayro-interpret | 0.8.0 (crates.io) | Apache-2.0 OR MIT | no (src and Cargo.toml byte-identical to upstream) | `vendor/hayro-interpret` |

- `patches/hayro/README.md` records the .crate sha256: hayro-syntax `6cdc818d...2026`, hayro-interpret `ac9df074...ffbe`.
- Planned patches P1-P6 are all "planned" status in that README. None are applied yet.
- The `[patch.crates-io]` wiring is in the root `Cargo.toml`. `hayro` 0.8.0 is used from crates.io.

## (b) Fonts, CMaps and ICC data embedded in the production graph

Resolved features (`cargo metadata`, wasm32): `hayro-interpret` has `embed-fonts`. `hayro-cmap` has no features, so **CMaps are not embedded in the wasm**. Since NEW-88 the 63 predefined CMaps ship as separate files in the npm package (`packages/sdk/cmaps/`), verbatim Adobe resources (adobe-type-tools/cmap-resources f5cf3bca7fdfeaceb77aa82847e974f2306c20b4, adobe-type-tools/mapping-resources-pdf 2dd5e53fb74a01718b9dfd448a0d1cce6fff2aa5 for the four `Adobe-*-UCS2` maps), BSD-3-Clause, sha256 in `cmaps/MANIFEST.json`, notice in `cmaps/LICENSE.txt` and `THIRD_PARTY_NOTICES.md`.

Fonts (feature `embed-fonts`, `vendor/hayro-interpret/src/font/standard_font.rs:173`, `include_bytes!`). These are the 14 Base-14 Foxit `.pfb` files:

- FoxitSans, FoxitSansBold, FoxitSansItalic, FoxitSansBoldItalic
- FoxitSerif, FoxitSerifBold, FoxitSerifItalic, FoxitSerifBoldItalic
- FoxitFixed, FoxitFixedBold, FoxitFixedItalic, FoxitFixedBoldItalic
- FoxitSymbol, FoxitDingbats

Origin: the README in `vendor/hayro-interpret/assets/` says they were "extracted from Pdfium". Copyright is Foxit Software, under the PDFium BSD-style license (`assets/LICENSE_FOXIT`). Obligation: keep the notice (already in `LICENSE_FOXIT`) when redistributing the wasm. Note: the Foxit font files themselves carry no separate font EULA in the repo; this is unverified.

ICC profiles (unconditional `include_bytes!`, not fonts):

- `CGATS001Compat-v2-micro.icc` (interpret and jpeg2000), CC0. Notice: `CGATS_LICENSE.txt`.
- `LAB.icc` (interpret and jpeg2000). README says it was generated with LCMS2 (interpret) or created manually with lcms (jpeg2000). Covered by the CC0 statement.
- `ProPhoto-v2-micro.icc` (hayro-jpeg2000 only), CC0 per its README and `assets/LICENSE.txt`.

## (c) Copied code in this repo

- `crates/` and `packages/`: grep for "copied from", "adapted from", "derived from", "ported from", "pdf.js", "pdfium", "mupdf", "pymupdf", "Copyright", "SPDX" found no hits.
- `docs/` hits are only prose describing PyMuPDF and the HDDN baseline. No code was copied.

## (d) Dev-only tools (segregated, never shipped)

- Root `devDependencies`: `playwright` 1.62.1 (Apache-2.0), plus transitive `playwright-core` (Apache-2.0) and `fsevents` (MIT, optional, macOS).
- `packages/sdk` `devDependencies`: `typescript` 5.9.3 (Apache-2.0).
- `packages/sdk` has zero runtime npm dependencies.
- Cargo dev-only: `pdf-writer` 0.15.0 (MIT OR Apache-2.0), `itoa`, `ryu`.
- Cargo build-only: `autocfg`, `rustversion`.
- PyMuPDF (AGPL/commercial) is an oracle, referenced in `docs/spec/v1.md` section 3.6. Nothing in `tests/oracles` exists yet. It is not in the Cargo or npm graph.

## (e) Gaps (unverified)

1. The built `.wasm` was not inspected. Font embedding is inferred from resolved features and `include_bytes!`, not from the binary. Build needs the memory queue. Not run in NEW-5.
2. Foxit font licence terms beyond the PDFium BSD-style notice are unverified. The `.pfb` origin is a PDFium extract, which matters under the "no C/C++ PDF engine" rule. Data, not engine code, but the lead should decide.
3. cargo-deny ran with no advisory check (`advisories` not in scope).
4. `guillotiere` declares `MIT/Apache-2.0`, which is not a valid SPDX expression. cargo-deny accepted it, but the expression is ambiguous and not verified.
5. The npm licence fields come from `node_modules/.pnpm` in the main checkout, not from `pnpm licenses list`. The worktree has no `node_modules`.
6. Three duplicate versions (`fearless_simd`, `miniz_oxide` 0.8.9 and 0.9.1, `syn` 2.x and 3.x) are warnings, not failures.
7. Playwright is in the lockfile but its browsers are not audited.
8. Review of hayro as a whole (bugs, security) is not in scope.

## Lead decision (2026-10-22)
Foxit Base-14 `.pfb` files are font data under the PDFium BSD-style notice, not C/C++ engine code: allowed in the shipped wasm. `LICENSE_FOXIT` must ship in the npm package notices (NEW-74).

## (f) Test-only fonts (NEW-49, never shipped)
`crates/foliark-text/tests/data/`: LiberationSans-Regular/Bold, LiberationSerif-Regular, LiberationMono-Regular (SIL OFL 1.1, `LICENSE-Liberation.txt`) and Carlito-Regular (SIL OFL 1.1, `OFL-Carlito.txt`), copied unmodified from HDDN `public/fonts/pdf/`. Used only by `cargo test -p foliark-text`; not in the production graph.

## (g) Source-font reuse and editor face (NEW-48)
No font asset added or shipped. The editor face is a rewrite of a program already inside the user's PDF (cmap, OS/2, name, post tables added; outlines untouched), not a redistributed font. `crates/foliark-engine/tests/font_registry.rs` reuses the NEW-49 Liberation Sans test font (OFL, above) and the DejaVu subset inside the HDDN fixture `unicode-glyph-widths.pdf` (DejaVu licence, see manifest). Subsetter compatibility: `subsetter` 0.2.6 output (glyf/loca/head/hhea/hmtx/maxp, no cmap, OS/2, name or post) is the shape the editor face repairs and the tests build from it; a later subsetter that starts keeping `cmap` is also handled (the cmap is kept only if it agrees with the glyph evidence).
