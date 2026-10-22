# Release process (NEW-75)

Status: prepared, never executed. Nothing is published, no remote exists, CI has never run. Publication is the owner gate NEW-78.

## Version policy
- Semver for `@foliark/sdk`. The public contract is `docs/spec/v1.md` and the exported TypeScript API.
- 0.x: breaking changes are allowed, each with a `CHANGELOG.md` entry that says what breaks and how to migrate. From 1.0 a breaking change needs a major.
- Patch: fixes, including security fixes and dependency updates. Minor (0.x) or minor (1.x): additive API, new typed codes, new supported content.
- A privacy defect fix (`SECURITY.md`) is always at least a patch release with its own entry.
- `packages/sdk/package.json` version, `CHANGELOG.md` heading and the workflow `version` input must agree; the workflow checks it.

## Who approves
- Owner decisions are placeholders until recorded: licence, copyright holder and contribution terms (CLA or DCO) and the private security channel (NEW-73), commercial and legal clearance (NEW-77), publication itself (NEW-78).
- An agent may fill the placeholders only after the owner records the decision in NEW-73. Nobody flips `"private": true` or runs the non-dry-run publish without NEW-78.

## Checklist
1. Owner placeholders resolved: `LICENSE` and `packages/sdk/LICENSE` and `NOTICE`, `license` in `package.json` and `Cargo.toml`, `CONTRIBUTING.md` terms, `SECURITY.md` channel and timeline. Lawyer review as the owner decides.
2. Clean tree, version bumped, `CHANGELOG.md` entry dated, `docs/support-matrix.md` current.
3. Full verification, each through `~/ai/bin/memory-queue` (list in `docs/release/candidate.md`): fmt, clippy, cargo test, `cargo deny check licenses bans sources`, wasm build, SDK build, TS parity, SDK tests, smoke (chromium, webkit), package consumers, browser specs, acceptance, budgets, notices check, fuzz smoke. Add a RustSec advisory check by hand.
4. Build and pack: `scripts/build-wasm.sh`, `pnpm build:sdk`, `tests/package-consumers/pack.sh dist-pkg`; record sha256 in `docs/release/candidate.md`.
5. `tools/release/dry-run.sh` exits 0 (bytes, notices, SBOM, `npm publish --dry-run` against an unreachable registry, docs). It also prints which placeholders still block publication.
6. Release workflow, `dry_run: true`, from the owner's repository. Review its artifacts: tarball, SBOM, SHA256SUMS.
7. Owner approves NEW-78. Run the workflow with `dry_run: false`; the `npm-release` environment needs manual approval and holds the only npm token (publish scope for `@foliark/sdk`). npm provenance (`--provenance`) attests the build.
8. Verify the installed artifact: install the published version in a clean project and run `tests/package-consumers/run.sh` against it; compare sha256 and the provenance statement.
9. Tag the release in git, record the receipt in the board.

## Rollback
- npm cannot republish a version. A bad release is withdrawn with `npm deprecate @foliark/sdk@<v> "<reason, use <v+1>>"` and fixed by a new patch release. `npm unpublish` only inside npm's window and only for a privacy defect that exposes data, by the owner.
- Consumers pin exact versions (HDDN pins the tarball hash). Revert them first, then publish the fix.
- A privacy defect: deprecate, follow `SECURITY.md` triage and disclosure, publish a fixed patch with a regression fixture.
- A failed workflow before publish changes nothing; delete the run's artifacts and fix forward.
