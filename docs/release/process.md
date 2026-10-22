# Release process (NEW-75)

Owner policy approved 2026-10-22 (NEW-73/NEW-78). Release execution and live registry
evidence belong to NEW-91; this document does not establish publication or CI state.
NEW-90 must recut the licensed candidate after NEW-84–NEW-89. The old rc2 tarball
is historical and cannot be published as the newly licensed release.

## Version policy
- Semver for Foliark SDK. The internal package is currently `@foliark/sdk`; NEW-90 migrates it to the proposed owner-scoped `@zilijonas/foliark` after verifying availability. The public contract is `docs/spec/v1.md` and the exported TypeScript API.
- 0.x: breaking changes are allowed, each with a `CHANGELOG.md` entry that says what breaks and how to migrate. From 1.0 a breaking change needs a major.
- Patch: fixes, including security fixes and dependency updates. Minor (0.x) or minor (1.x): additive API, new typed codes, new supported content.
- A privacy defect fix (`SECURITY.md`) is always at least a patch release with its own entry.
- `packages/sdk/package.json` version, `CHANGELOG.md` heading and the workflow `version` input must agree; the workflow checks it.

## Who approves
- Owner: Linas Jonas Zilinskas. Public license: AGPL-3.0-only plus separate commercial licensing; no outside code contributions initially. Source repository: `zilijonas/foliark`, public. Security channel: GitHub private vulnerability reporting, to be enabled and verified by NEW-91.
- The owner authorized public npm publication and direct PROD HDDN deployment before personal device tests. Follow NEW-90 → NEW-91 → NEW-92 → NEW-93; keep actual device evidence in NEW-64 pending until supplied. Do not repeat the recorded business or rollout approval questions.
- NEW-90 prepares publishable package metadata and verifies exact bytes; NEW-91 publishes only that qualified candidate. The root workspace stays private. Any required login/2FA or provider-side approval is completed by the owner without sharing credentials in chat.

## Checklist
1. Owner decisions and source files agree: `LICENSE`, `LICENSING.md`, `COMMERCIAL-LICENSING.md`, package copies/metadata/notices, `Cargo.toml`, `CONTRIBUTING.md`, and `SECURITY.md`. Preserve third-party notices and exact corresponding source for compiled artifacts. Commercial customer agreements are separate; no general proprietary-use grant is implied.
2. Clean tree, version bumped, `CHANGELOG.md` entry dated, `docs/support-matrix.md` current.
3. Full verification, each through `~/ai/bin/memory-queue` (list in `docs/release/candidate.md`): fmt, clippy, cargo test, `cargo deny check licenses bans sources`, wasm build, SDK build, TS parity, SDK tests, smoke (chromium, webkit), package consumers, browser specs, acceptance, budgets, notices check, fuzz smoke. Add a RustSec advisory check by hand.
4. Build and pack: `scripts/build-wasm.sh`, `pnpm build:sdk`, `tests/package-consumers/pack.sh dist-pkg`; record sha256 in `docs/release/candidate.md`.
5. `tools/release/dry-run.sh` exits 0 (bytes, notices, SBOM, `npm publish --dry-run` against an unreachable registry, docs). It also prints which placeholders still block publication.
6. Release workflow, `dry_run: true`, from the owner's repository. Review its artifacts: tarball, SBOM, SHA256SUMS.
7. Execute NEW-91 using the recorded NEW-78 authorization. NEW-90 must update the workflow's old token-only publishing assumptions and package identity, using supported npm trusted publishing where practical; any first-publish login/2FA remains local to the owner. Verify provenance when supported. Never publish an unqualified re-build instead of the checked tarball.
8. Verify the installed artifact: install the published version in a clean project and run `tests/package-consumers/run.sh` against it; compare sha256 and the provenance statement.
9. Tag the release in git, record the receipt in the board.

## Rollback
- npm cannot republish a version. A bad release is withdrawn with `npm deprecate @foliark/sdk@<v> "<reason, use <v+1>>"` and fixed by a new patch release. `npm unpublish` only inside npm's window and only for a privacy defect that exposes data, by the owner.
- Consumers pin exact versions and registry integrity (NEW-92 replaces HDDN's old absolute tarball path). Revert them first, then publish the fix.
- A privacy defect: deprecate, follow `SECURITY.md` triage and disclosure, publish a fixed patch with a regression fixture.
- A failed workflow before publish changes nothing; delete the run's artifacts and fix forward.
