# Release candidate: @zilijonas/foliark 0.1.4 (NEW-110/113: paragraph style segments, inline run styles)

Qualified on 2026-10-24 on the owner's Mac mini (same toolchain as 0.1.0). Engine and API changes versus 0.1.3 (`CHANGELOG.md`, ADR 0013): `ParagraphInfo.segments`, `looks` and `runs` and `EditPreview.styles` (additive) report a paragraph's styles, and an edit keeps the bold, italic, size and colour of the words it does not touch (new words take their neighbours' style); with a caller font a bold or italic run takes the matching face or the edit refuses `EDIT_NO_FONT` (NEW-110/113). Not published by an agent: the owner publishes this exact tarball (`npm publish`, local). Source correspondence: tag `v0.1.4` on the commit that carries this record; the tarball was built from the source commit below, and the record commit differs from it only by documentation and `tools/bench/budgets.json` (not in the package). `tools/release/dry-run.sh` reads the first recorded sha256 in this file, so the 0.1.4 hash stays in this section, above the 0.1.3 record. `release/0.1.4` is `main` (0.1.3) plus the merge of `feat/edit-inline-runs` (which holds `feat/paragraph-segments`). The wasm gzip budget was re-based on this build (2,232,320 B cap exceeded by 8,482 B, 0.38%: 0.1.4 is 2,240,802 B gzip, new cap 2,352,843 B), x1.05 rule, lead decision, no timing budget changed; raw wasm 5,876,192 B is inside the 0.1.3 raw cap.

| | |
|---|---|
| file | `dist-pkg/zilijonas-foliark-0.1.4.tgz` (gitignored) |
| sha256 | `e88c0c4a58462df8b964227c554edb38eab7152c8c295fa4e5a357d9d0537949` |
| size | 3,453,390 B, 282 entries (0.1.3: 276; the 6 new entries are the generated `ParagraphSegment`, `StyleSpan` and `TextLook` types) |
| built from | foliark `1f436d4` (`chore(release): bump ... 0.1.4`, clean tree before and after); `scripts/build-wasm.sh && pnpm build:sdk && tests/package-consumers/pack.sh dist-pkg`; packing three times gave the same sha256, and a repack from `e5d2f39` (the budget re-base, sources otherwise identical) gave the same tarball sha256 |
| wasm | `wasm/foliark_wasm_bg.wasm` sha256 `26b4f59361f5045dc47165a8cdca84ded41fc1081f9e863e58b27a7c821cf8d7`, 5,876,192 B raw, 2,240,802 B gzip -9 (0.1.3: `04040131...`, 5,850,003 B) |

Checks on `1f436d4` (one supervised chain through `~/ai/bin/memory-queue`, resumed three times after two other chats' yield requests, no step replayed after it passed; fuzz smoke self-queued, run unwrapped):

| Check | Command | Exit | Result |
|---|---|---|---|
| format, lint | `cargo fmt --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0, 0 | |
| Rust tests | `cargo test --workspace --locked --no-fail-fast` | 0 | 434 passed, 0 failed, 3 ignored over 59 targets, including `strict_native` |
| wasm and SDK build | `scripts/build-wasm.sh && pnpm build:sdk` | 0 | sha256 above |
| TS parity | `scripts/check-ts-parity.sh` | 0 | in sync (92 files) |
| SDK tests | `pnpm --filter @zilijonas/foliark exec vitest run --maxWorkers=1 --no-file-parallelism` | 0 | 88 passed (12 files) |
| smoke | `pnpm smoke chromium`, `pnpm smoke webkit` | 0, 0 | |
| browser specs | `tests/browser/run.sh` | 0 | 33 passed (chromium, firefox, webkit) |
| pack x3 | `tests/package-consumers/pack.sh dist-pkg` | 0, 0, 0 | identical sha256 |
| package consumers | `tests/package-consumers/run.sh` | 0 | 6 builds ok, 0 external requests |
| oracle controls | `tests/oracles/.venv/bin/python tests/oracles/controls.py` | 0 | leaky caught 30/30, safe passed 14/14; fidelity fail cases caught 7/7, pass 3/3 |
| acceptance | `SKIP_BUILD=1 tests/integration/acceptance/run.sh` | 0 | 88 cases, 528 results: SAFE 433, EXPLAINED 35, REFUSED 60; negative controls 125, missed 0; cancellation 27 attempts, 0 findings |
| fuzz smoke | `FUZZ_SECS=30 tools/fuzz-smoke/run.sh` | 0 | 7 targets x 30 s, all clean |
| budgets | `node --test tools/bench/check-budgets.test.mjs`; fresh `sdk.mjs --extended` x2 + `scans-sdk.mjs` (chromium) through `check-budgets.mjs` | 0, 0 | before the re-base: 39 ok, 38 webkit skipped, 1 failing (wasm gzip 2,240,802 / max 2,232,320); after the re-base (`e5d2f39`) on the same fresh files: 40 ok, 38 skipped, 0 failing |
| notices | `node scripts/gen-notices.mjs --check` | 0 | up to date |
| licences | `cargo deny check licenses bans sources` | 0 | ok (no RustSec advisory check run) |
| export size | `cargo build --release -p foliark-engine --example export_size`; `tools/bench/export-size.py --check` | 0, 0 | Foliark <= PyMuPDF on every exportable fixture |
| release dry run | `tools/release/dry-run.sh` | 0 | tarball bytes equal the recorded sha256, manifest 0.1.4, 282 entries, notices, SBOM (102 components), `SHA256SUMS`, package README names tag `v0.1.4`, `npm publish --dry-run` against an unreachable registry |

Owner publish command, from the repository root (after the `v0.1.4` tag exists on the record commit; rebuild with the commands above if the tarball is gone and compare the sha256):
`npm publish ./dist-pkg/zilijonas-foliark-0.1.4.tgz --access public`

---

# Release candidate: @zilijonas/foliark 0.1.3 (NEW-105/106: Word edit fixes)

Qualified on 2026-10-24 on the owner's Mac mini (same toolchain as 0.1.0). Engine changes versus 0.1.2 (`CHANGELOG.md`, ADR 0012): removing text no longer empties the glyphs of untouched text drawn by a font twin that shares an embedded program (Word's WinAnsi TrueType plus Identity-H Type0 pair; edits and text redactions, NEW-105), and an edit of a block holding a bold heading over a regular body splits the paragraph into style segments, keeps the heading and body styles and stays inside the margin (NEW-106). Not published by an agent: the owner publishes this exact tarball (`npm publish`, local). Source correspondence: tag `v0.1.3` on the commit that carries this record; the tarball was built from the source commit below, and the record commit differs from it only by documentation. `tools/release/dry-run.sh` reads the first recorded sha256 in this file, so the 0.1.3 hash stays in this section, above the 0.1.2 record. The branches `fix/edit-glyph-loss` and `fix/edit-styles` were merged into `release/0.1.3` (conflicts in `CHANGELOG.md`, `generate.py` and `manifest.json` only, both sides kept; manifest: 46 fixtures, `tests/fixtures/verify.py` 0 errors). The wasm byte budgets were re-based on this build (0.1.2: 5,835,122 B, now 5,850,003 B; the NEW-83 baseline plus 5% was used up), x1.05 rule, no timing budget changed.

| | |
|---|---|
| file | `dist-pkg/zilijonas-foliark-0.1.3.tgz` (gitignored) |
| sha256 | `444054bc0d3bad526de4a7843e21539f089dc67bbdbb9f7d2c3066fd3773b32b` |
| size | 3,442,484 B, 276 entries (same entry list as 0.1.0) |
| built from | foliark `6b31d82` (`perf(bench): re-base wasm byte budgets ...`, clean tree before and after); `scripts/build-wasm.sh && pnpm build:sdk && tests/package-consumers/pack.sh dist-pkg`; packing twice gave the same sha256, and a first build from `cccb5c2` (the 0.1.3 bump, identical sources except `tools/bench/budgets.json`) gave the same wasm and tarball sha256 |
| wasm | `wasm/foliark_wasm_bg.wasm` sha256 `04040131310bd66c78a9fddebbb4be63735105b7dcd104ad8c07887a680c4a0a`, 5,850,003 B raw (0.1.2: `90820663...`) |

Checks on `6b31d82` (one reservation each through `~/ai/bin/memory-queue`; fuzz smoke self-queued, run unwrapped):

| Check | Command | Exit | Result |
|---|---|---|---|
| format, lint | `cargo fmt --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0, 0 | |
| Rust tests | `cargo test --workspace --locked --no-fail-fast` | 0 | 422 passed, 0 failed, 3 ignored over 58 targets, including `strict_native` |
| wasm and SDK build | `scripts/build-wasm.sh && pnpm build:sdk` | 0 | sha256 above |
| TS parity | `scripts/check-ts-parity.sh` | 0 | in sync (89 files) |
| SDK tests | `pnpm --filter @zilijonas/foliark test` | 0 | 87 passed |
| smoke | `pnpm smoke chromium`, `pnpm smoke webkit` | 0, 0 | |
| browser specs | `tests/browser/run.sh` | 0 | chromium, firefox, webkit |
| pack twice | `tests/package-consumers/pack.sh dist-pkg` x2 | 0, 0 | identical sha256 |
| package consumers | `tests/package-consumers/run.sh` | 0 | whole matrix passed |
| oracle controls | `tests/oracles/.venv/bin/python tests/oracles/controls.py` | 0 | leaky caught 29/29, safe passed 13/13; fidelity fail cases caught 7/7, pass 3/3 (GLYPHS and the style fixture included) |
| acceptance | `SKIP_BUILD=1 tests/integration/acceptance/run.sh` | 0 | 87 cases, 522 results: SAFE 427, EXPLAINED 35, REFUSED 60; negative controls 123, missed 0; cancellation 27 attempts, 0 findings |
| fuzz smoke | `FUZZ_SECS=30 tools/fuzz-smoke/run.sh` | 0 | 7 targets x 30 s, all clean |
| budgets | `node --test tools/bench/check-budgets.test.mjs`; fresh `sdk.mjs --extended` x2 + `scans-sdk.mjs` (chromium) through `check-budgets.mjs` | 0, 0 | 40 ok, 38 webkit skipped, 0 failing (first run with the old wasm caps: 2 failing, rawBytes and cold download bytes, fixed by the re-base above) |
| notices | `node scripts/gen-notices.mjs --check` | 0 | up to date |
| licences | `cargo deny check licenses bans sources` | 0 | ok (no RustSec advisory check run) |
| export size | `cargo build --release -p foliark-engine --example export_size`; `tools/bench/export-size.py --check` | 0, 0 | Foliark <= PyMuPDF on every exportable fixture |
| owner files (counts only, files read in place, export deleted) | packed SDK, `airpods 4.pdf`: 38 paragraphs through `prepareParagraph` + `layoutEdit`, with and without a registered font; `IX-966_RedakcijaNr_18 (1).pdf`: edit of the page-1 article 1 body (3 rows, bold heading + regular body), then `fidelity.py` | 0 | airpods 38 of 38: 0 refused, 0 not fitting, 0 re-broken. IX-966: heading spans identical (font, size, box), body font unchanged (regular), right edge 569.40 in, 531.25 out (no overshoot), 2 body spans changed; fidelity 0 findings over 20 pages, GLYPHS 0 |
| release dry run | `tools/release/dry-run.sh` | 0 | tarball bytes equal the recorded sha256, manifest 0.1.3, 276 entries, notices, SBOM (102 components), `SHA256SUMS`, package README names tag `v0.1.3`, `npm publish --dry-run` against an unreachable registry |

Owner publish command, from the repository root (after the `v0.1.3` tag exists on the record commit; rebuild with the commands above if the tarball is gone and compare the sha256):
`npm publish ./dist-pkg/zilijonas-foliark-0.1.3.tgz --access public`

---

# Release candidate: @zilijonas/foliark 0.1.2 (NEW-100/102/104: real iOS and Word PDFs)

Qualified on 2026-10-24 on the owner's Mac mini (same toolchain as 0.1.0). Engine changes versus 0.1.1 (`CHANGELOG.md`, ADR 0011): text of fonts whose ToUnicode maps nothing (iOS Quartz) is read from the embedded font's own cmap
(NEW-100), zero-width combining marks take the box of their base glyph so native redaction no longer refuses those pages (NEW-102), and an unchanged paragraph fits its edit box, with a decomposed fallback for a missing precomposed letter
(NEW-104). Not published by an agent: the owner publishes this exact tarball (`npm publish`, local). Source correspondence: tag `v0.1.2` on the commit that carries this record; the tarball was built from the source commit
below, and the record commit differs from it only by documentation. `tools/release/dry-run.sh` reads the first recorded sha256 in this file, so the 0.1.2 hash stays in this section, above the 0.1.1 record.

| | |
|---|---|
| file | `dist-pkg/zilijonas-foliark-0.1.2.tgz` (gitignored) |
| sha256 | `328a6c9ed85f62f58c9fdd06b7fb5f360bc89c4bcabafab618a6b3ede63a0e16` |
| size | 3,435,939 B, 276 entries (same entry list as 0.1.0) |
| built from | foliark `2f68ff3` (`chore(release): bump ... 0.1.2`, clean tree before and after); `scripts/build-wasm.sh && pnpm build:sdk && tests/package-consumers/pack.sh dist-pkg`; packing twice gave the same sha256 |
| wasm | `wasm/foliark_wasm_bg.wasm` sha256 `90820663e259f4dfb22996577140097cfcc968976ffc2c757ec89bc4bf3b1c8d`, 5,835,122 B raw (0.1.1: `4da12fce...`) |

Checks on `2f68ff3` (one reservation each through `~/ai/bin/memory-queue`; fuzz smoke self-queued, run unwrapped):

| Check | Command | Exit | Result |
|---|---|---|---|
| format, lint | `cargo fmt --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0, 0 | |
| Rust tests | `cargo test --workspace --locked --no-fail-fast` | 0 | 417 passed, 0 failed, 3 ignored over 57 targets, including `strict_native` |
| wasm and SDK build | `scripts/build-wasm.sh && pnpm build:sdk` | 0 | sha256 above |
| TS parity | `scripts/check-ts-parity.sh` | 0 | in sync |
| SDK tests | `pnpm --filter @zilijonas/foliark test` | 0 | 87 passed |
| smoke | `pnpm smoke chromium`, `pnpm smoke webkit` | 0, 0 | |
| browser specs | `tests/browser/run.sh` | 0 | 33 passed (chromium, firefox, webkit) |
| pack twice | `tests/package-consumers/pack.sh dist-pkg` x2 | 0, 0 | identical sha256 |
| package consumers | `tests/package-consumers/run.sh` | 0 | 6 builds ok, 0 external requests |
| oracle controls | `tests/oracles/.venv/bin/python tests/oracles/controls.py` | 0 | leaky caught 28/28, safe passed 12/12; fidelity fail cases caught 5/5, pass 2/2 |
| acceptance | `SKIP_BUILD=1 tests/integration/acceptance/run.sh` | 0 | 85 cases, 510 results: SAFE 415, EXPLAINED 35, REFUSED 60; negative controls 119, missed 0; cancellation 27 attempts, 0 findings |
| fuzz smoke | `FUZZ_SECS=30 tools/fuzz-smoke/run.sh` | 0 | 7 targets x 30 s, all clean |
| budgets | `node --test tools/bench/check-budgets.test.mjs`; fresh `sdk.mjs --extended` + `scans-sdk.mjs` (chromium) through `check-budgets.mjs` | 0, 0 | 40 ok, 38 webkit skipped, 0 failing |
| notices | `node scripts/gen-notices.mjs --check` | 0 | up to date |
| licences | `cargo deny check licenses bans sources` | 0 | ok (no RustSec advisory check run) |
| export size | `cargo build --release -p foliark-engine --example export_size`; `tools/bench/export-size.py --check` | 0, 0 | Foliark <= PyMuPDF on every exportable fixture |
| release dry run | `tools/release/dry-run.sh` | 0 | tarball bytes equal the recorded sha256, manifest 0.1.2, notices, SBOM, `SHA256SUMS`, package README names tag `v0.1.2`, `npm publish --dry-run` against an unreachable registry |

Owner publish command, from the repository root (after the `v0.1.2` tag exists on the record commit; rebuild with the commands above if the tarball is gone and compare the sha256):
`npm publish ./dist-pkg/zilijonas-foliark-0.1.2.tgz --access public`

---

# Release candidate: @zilijonas/foliark 0.1.1 (NEW-94: HDDN public-corpus engine fixes + npm page)

Qualified on 2026-10-22 on the owner's Mac mini (same toolchain as 0.1.0). Engine changes versus 0.1.0 (`CHANGELOG.md`, ADR 0010, hayro patches P20 and P21): an image stream is bounded by its declared
size (an 81 MP scan imports), the budget scan counts Flate images instead of holding them, the first draw of such a scan peaks at +230 MB instead of +510 MB, NoRotate annotations flatten and rasterize
upright on rotated pages, and flattened appearances stay separate objects in the staged copy of an export. The npm page and `package.json` metadata are the earlier 0.1.1 edits. Not published by an agent:
the owner publishes this exact tarball (`npm publish`, local, no provenance, as for 0.1.0). Source correspondence: tag `v0.1.1` on the commit that carries this record (it differs from the built commit only by this record).
`tools/release/dry-run.sh` reads the first recorded sha256 in this file, so the 0.1.1 hash stays in this section, above the 0.1.0 record. The earlier docs-only 0.1.1 tarball (sha256 `49429ab4...`) is superseded and must not be published.

| | |
|---|---|
| file | `dist-pkg/zilijonas-foliark-0.1.1.tgz` (gitignored) |
| sha256 | `612a09a290917ca4cc255c09dcf2bd3a54fd88cbc4e0bfacc9944be700eb8e92` |
| size | 3,433,581 B, 276 entries (same entry list as 0.1.0) |
| built from | foliark `26e5d0bc51807de48bc6ce83617a9ae1fe0599e1`, clean tree; `scripts/build-wasm.sh && pnpm build:sdk && tests/package-consumers/pack.sh dist-pkg`; packing twice gave the same sha256 |
| wasm | `wasm/foliark_wasm_bg.wasm` sha256 `4da12fce42c395292840c2ee582218b3b1c9e7f88fd7df7854f8382611e795af` (0.1.0: `0ecce6f8...`, changed by the engine fixes) |
| HDDN evidence | `pnpm run test:pdf:isolated` with this build in the 1.5 GiB container, synthetic and public corpus (33 cases): 32 clean; open item `public-cropped-rotated` image-only page 3 (see Limits). The 81 MP scan (`public-mixed-multipage`) loads all 6 pages; the form (`public-form-libreoffice`) is a typed refusal with a passing image-only export |

Checks on `26e5d0b` (one 4 GB queue reservation each; fuzz smoke self-queued, run unwrapped):

| Check | Command | Exit | Result |
|---|---|---|---|
| wasm and SDK build | `scripts/build-wasm.sh && pnpm build:sdk` | 0 | sha256 above |
| pack twice | `tests/package-consumers/pack.sh dist-pkg` x2 | 0, 0 | identical sha256 |
| notices | `node scripts/gen-notices.mjs --check` | 0 | up to date |
| format, lint | `cargo fmt --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0, 0 | |
| Rust tests | `cargo test --workspace --locked --no-fail-fast` | 0 | 412 passed, 0 failed, 3 ignored over 56 targets, including `strict_native` |
| TS parity | `scripts/check-ts-parity.sh` | 0 | in sync |
| licences | `cargo deny check licenses bans sources` | 0 | ok (no RustSec advisory check run) |
| SDK tests | `pnpm --filter @zilijonas/foliark test` | 0 | passed |
| smoke | `pnpm smoke chromium`, `pnpm smoke webkit` | 0, 0 | |
| package consumers | `tests/package-consumers/run.sh` | 0 | 6 builds ok, 0 external requests |
| browser specs | `tests/browser/run.sh` | 0 | 33 passed |
| oracle controls | `tests/oracles/.venv/bin/python tests/oracles/controls.py` | 0 | leaky controls caught, safe passed |
| acceptance | `SKIP_BUILD=1 tests/integration/acceptance/run.sh` | 0 | 81 cases, 486 results: SAFE 391, EXPLAINED 35, REFUSED 60; negative controls 111, missed 0; cancellation 27 attempts, 0 findings |
| fuzz smoke | `FUZZ_SECS=30 tools/fuzz-smoke/run.sh` | 0 | 7 targets x 30 s, all clean |
| budgets | `node --test tools/bench/check-budgets.test.mjs`; fresh `sdk.mjs --extended` + `scans-sdk.mjs` (chromium) through `check-budgets.mjs` | 0, 0 | 40 chromium budgets ok, 38 webkit skipped, 0 failing |
| export size | `cargo build --release -p foliark-engine --example export_size`; `tools/bench/export-size.py --check` | 0, 0 | Foliark <= PyMuPDF on every exportable fixture |

| release dry run | `tools/release/dry-run.sh` | 0 | tarball bytes equal the recorded sha256, 276 entries, manifest 0.1.1, notices, SBOM (102 components), `SHA256SUMS`, package README names tag `v0.1.1`, `npm publish --dry-run` against an unreachable registry |
| npm dry run | `npm publish "$PWD/dist-pkg/zilijonas-foliark-0.1.1.tgz" --dry-run --access public` | 0 | `@zilijonas/foliark@0.1.1`, 276 files, shasum `baffd7b9b1c81fac7dafaea56cfba9a695413315`; nothing published |

Limits of this candidate: HDDN's pixel check of the image-only export of `public-cropped-rotated` page 3 reports 0.86 percent changed (budget 0.5). The remaining difference is horizontal 1 pt label borders:
Poppler draws them pixel-snapped, Foliark's rasterizer anti-aliases them over two rows, and the 2x raster downsampled by Poppler does not match on that page's half-pixel alignment (pages 1, 2 and 4 pass at 0.26 to 0.42
percent). It is not an engine content loss: with annotations removed the page differs by the same 0.75 percent. Owner decision needed for the HDDN gate (comparator or allowance).

Owner publish command, from the repository root:
`npm publish ./dist-pkg/zilijonas-foliark-0.1.1.tgz --access public`
(after the `v0.1.1` tag exists on the source commit; rebuild with the commands above if the tarball is gone and compare the sha256).

---

# Release candidate: @zilijonas/foliark 0.1.0 (NEW-90)

Superseded by 0.1.1 above for the npm package page; the engine and wasm are the same.

Licensed public candidate, qualified on 2026-10-22 on the owner's Mac mini (macOS 26.6.2, arm64). AGPL-3.0-only plus separate
commercial licensing (`LICENSING.md`). Not yet published: NEW-91 publishes exactly this tarball, locally by the owner (`npm publish`,
no provenance: provenance needs CI OIDC; trusted publishing is configured on npmjs.com after the package exists, then `release.yml`
publishes later versions). The earlier rc2 record (`@foliark/sdk`, UNLICENSED, sha `4ead7f33...`) is superseded and must not be installed.

## Artifact
| | |
|---|---|
| file | `dist-pkg/zilijonas-foliark-0.1.0.tgz` (gitignored; rebuild with the commands below) |
| sha256 | `7f66ad820e8839112ec903019eeef1f20cecf6ff8d64fe1604e5aa79624dbcd6` |
| size | 3,429,852 B, 276 entries (no runtime dependencies, no fixtures/tests/maps/PDFs) |
| built from | foliark git `30762cfab1d0baceb43c86810e90d68dfa2e6ff2` (`30762cf`), clean tree before and after the build (the only change a pack makes is refreshing the tracked copy `packages/sdk/LICENSING.md` from the root file; that copy was synced in the record commit and a repack from it gives the same sha256) |
| wasm | `wasm/foliark_wasm_bg.wasm` sha256 `0ecce6f8db9d0a1d0f02543ef326a62c3cf70ea20810c243ba54f6fa72f132d2`, 5,824,944 B raw, 2,219,561 B `gzip -9 -n` |
| toolchain | rustc/cargo 1.99.1 (`rust-toolchain.toml`), wasm32-unknown-unknown, wasm-bindgen 0.2.129, wasm-opt (binaryen) 132, Node 26.8.1, pnpm 11.9.0, npm 11.19.0, macOS 26.6.2 arm64 (Apple M4) |
| reproducibility | packing twice from the same build gave the same sha256. Same machine and toolchain only; the wasm embeds the builder's cargo registry paths in panic locations (`/Users/agent/.cargo/registry/...`, a service account name, no workspace or personal path), so cross-machine byte identity is not claimed |
| manifest | name `@zilijonas/foliark`, version 0.1.0, license `AGPL-3.0-only`, `private` absent, `publishConfig.access` public, author Linas Jonas Zilinskas, repository/homepage/bugs `github.com/zilijonas/foliark`; root workspace stays `private: true` |
| SBOM | `dist-pkg/zilijonas-foliark-0.1.0.cdx.json` (CycloneDX 1.5, 102 components, root hash = tarball), `dist-pkg/SHA256SUMS` |

Contents: `package.json`, `README.md`, `LICENSE` (AGPL-3.0, identical to the repository file), `LICENSING.md`, `COMMERCIAL-LICENSING.md`, `LICENSE_FOXIT`, `NOTICE`,
`THIRD_PARTY_NOTICES.md`, `SUPPORT_MATRIX.md`, `dist/` (JS and `.d.ts`, generated DTOs), `wasm/` (engine, glue), `cmaps/` (63 CMap files, `LICENSE.txt`, `MANIFEST.json`).

## Source correspondence (AGPL)
Public source: https://github.com/zilijonas/foliark. Version 0.1.0 corresponds to tag `v0.1.0`, created by NEW-91 after the release PR is merged, on the
commit that carries this record (it differs from `30762cf` only by documentation and the synced `packages/sdk/LICENSING.md` copy, neither of which changes the tarball).
The tag holds the Rust workspace, the vendored hayro fork (`vendor/hayro-syntax`, `vendor/hayro-interpret`) and its patch register (`patches/hayro/README.md`),
`Cargo.lock`, `rust-toolchain.toml`, the CMap pack and its build tool, the SDK, notice generation and the build steps (`docs/development.md`, below). `LICENSING.md` and the package
`README.md` state this; `tools/release/dry-run.sh` checks that the package README names the tag and that the fork and patch register exist. Third-party licences:
`THIRD_PARTY_NOTICES.md`, `NOTICE`, `LICENSE_FOXIT`, `cmaps/LICENSE.txt`, `docs/provenance/`. `release.yml` for later releases refuses a dispatch that is not from the tag `vX.Y.Z`.

## Rebuild
```sh
scripts/build-wasm.sh && pnpm build:sdk && tests/package-consumers/pack.sh dist-pkg      # each through ~/ai/bin/memory-queue
```

## Verification on `30762cf` (clean tree; one 4 GB queue reservation for everything except the fuzz smoke, which self-reserves; logs outside the repo)
| Check | Command | Exit | Result |
|---|---|---|---|
| install | `pnpm install --frozen-lockfile` | 0 | lockfile unchanged by the rename |
| wasm build | `scripts/build-wasm.sh` | 0 | sizes above |
| SDK build | `pnpm build:sdk` | 0 | |
| pack twice | `tests/package-consumers/pack.sh` x2 | 0, 0 | identical sha256 |
| notices | `node scripts/gen-notices.mjs --check` | 0 | up to date |
| format | `cargo fmt --check` | 0 | |
| lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | |
| Rust tests | `cargo test --workspace --locked --no-fail-fast` | 0 | 408 passed, 0 failed, 3 ignored over 56 test targets, including `strict_native` |
| TS parity | `scripts/check-ts-parity.sh` | 0 | 89 files in sync |
| licences | `cargo deny check licenses bans sources` | 0 | bans, licenses, sources ok (no RustSec advisory check run) |
| SDK tests | `pnpm --filter @zilijonas/foliark test` | 0 | 87 passed |
| smoke | `pnpm smoke chromium`, `pnpm smoke webkit` | 0, 0 | render 200x100 ok, watchdog hang test ok in both |
| package consumers | `tests/package-consumers/run.sh` | 0 | 6 builds ok (vanilla, Vite, esbuild at `/` and `/app/`), tarball installed, not linked, 0 external requests |
| browser specs | `tests/browser/run.sh` | 0 | 33 passed (11 specs x chromium, firefox, webkit: CSP, offline, prepared-offline CMaps) |
| oracle controls | `tests/oracles/.venv/bin/python tests/oracles/controls.py` | 0 | leaky controls caught 25/25, safe passed 9/9; fidelity fail cases caught 5/5, pass 2/2 |
| acceptance | `SKIP_BUILD=1 tests/integration/acceptance/run.sh` (not self-reserving, inside the one reservation) | 0 | 81 cases, 486 results: SAFE 391, EXPLAINED 35, REFUSED 60, 0 blockers; negative controls 111, missed 0; cancellation 27 attempts, 0 findings; installed the packed tarball |
| fuzz smoke | `FUZZ_SECS=30 tools/fuzz-smoke/run.sh` (self-queued, run unwrapped) | 0 | 7 targets x 30 s, all clean (about 13.7 M runs on the last) |
| budgets | `node --test tools/bench/check-budgets.test.mjs`; fresh `sdk.mjs --extended` + `scans-sdk.mjs` (chromium) through `check-budgets.mjs` | 0, 0, 0 | 40 chromium budgets ok, 38 webkit skipped (no webkit bench), 0 failing |
| export size | `cargo build --release -p foliark-engine --example export_size`; `tools/bench/export-size.py --check` | 0, 0 | Foliark <= PyMuPDF on every exportable fixture (native-30p 27,500 vs 28,747 B); `compressed-stream` and `oversized-image` refused by design |
| release dry run | `tools/release/dry-run.sh` | 0 | bytes equal the recorded sha256, notices, SBOM, `npm publish --dry-run` against an unreachable registry |
| npm dry run | `npm publish "$PWD/dist-pkg/zilijonas-foliark-0.1.0.tgz" --dry-run --access public` | 0 | 276 files, 3.4 MB, name `@zilijonas/foliark` unclaimed (`npm view` 404), `npm whoami` = zilijonas; nothing published |

## Privacy and secrets scan
Source tree (690 tracked files) and the unpacked tarball (276 files): no tokens, keys, `.env` files, `_authToken`, PDFs or fixtures in the package; no owner email, no personal
absolute path in the tree (one local Codex output path in `docs/research/hddn-baseline.md` was replaced by a placeholder); fixture PDFs are synthetic by manifest
(`tests/fixtures/manifest.json`); the only passwords in the tree are the documented fixture passwords of the synthetic encrypted PDFs. Findings kept: the wasm embeds
cargo registry paths under the builder's `/Users/agent/.cargo` (generic account name); the git history carries the commit author email of the agent identity (`Agent Homie`), which becomes public with the repository.

## Limits
- Real iPhone, iPad, Safari and Android: not tested (post-PROD, NEW-64). Playwright WebKit is not Safari. See `docs/support-matrix.md`.
- Budgets were frozen on one machine; webkit budgets were not re-run. The candidate was verified on the build machine only.
- No RustSec advisory check; Foxit font terms beyond the PDFium notice are documented, not legally cleared.
- npm provenance is not attached to 0.1.0 (local publication); later releases use trusted publishing once configured.
- Publication, the git tag, GitHub private vulnerability reporting and trusted publishing configuration are NEW-91 steps.
