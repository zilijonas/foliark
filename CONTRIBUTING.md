# Contributing to Foliark

External contributions are not accepted until the owner selects the license and contribution terms (CLA or DCO, NEW-73).
Please do not open pull requests or send patches before then. Until it is decided the repository is private and the licence is a placeholder (`LICENSE`).
Options under discussion: `docs/business/licensing-options.md`.

The rest of this file is how work is done today, so that it is ready when the terms are set.

## Setup
Toolchain, commands and the memory queue for heavy runs: `docs/development.md`. The contract is `docs/spec/v1.md` (frozen; a change needs an ADR in `docs/adr/`).
Always pass `--locked`; `Cargo.lock` and `pnpm-lock.yaml` are committed.

## Quality bar
- Tests prove behaviour that could break. No tests that mirror the implementation, no trivial getter tests.
- Privacy-relevant behaviour (redaction, sanitizing, export) needs an independent oracle (`tests/oracles`) and a negative control showing the oracle catches the defect.
- Smallest complete change: reuse before adding, no speculative abstraction. Keep validation and error handling.
- No C/C++ PDF engine in the shipped graph. Changes to the vendored hayro crates are additive, upstreamable and get a row in `patches/hayro/README.md`.
- Every change that moves a claim updates the matching document (`docs/support-matrix.md`, `CHANGELOG.md`, notices via `node scripts/gen-notices.mjs`).
- Report what you ran as a receipt: commit, exact commands, exit codes, counts, limitations. Never claim a check you did not run.

## Commits
Conventional commits: `type(scope): summary (TASK-ID)`, for example `fix(redact): drop inherited clip on form XObjects (NEW-33)`. One logical change per commit, body says why.
Contribution terms, once chosen (CLA or DCO sign-off), will be stated here.

## Security issues
Do not file them as ordinary issues. See `SECURITY.md`.
