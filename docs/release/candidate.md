# Release candidate: @zilijonas/foliark 0.1.0 (NEW-90)

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
