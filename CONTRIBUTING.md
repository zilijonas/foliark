# Contributing to Foliark

External code contributions are not accepted initially. Only Linas Jonas Zilinskas
and agents acting for him contribute code at this stage. Please do not send patches
or open code pull requests until an updated contribution policy is published.

The owner selected AGPL-3.0-only plus separate commercial licensing on 2026-10-22
(NEW-73). No CLA or DCO intake process is active. Accepting outside code later
requires an owner-approved rights/contribution policy before the first contribution.
Bug reports and feature suggestions are welcome; security issues use `SECURITY.md`.

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
External-contribution terms will be stated here if the owner opens code contributions.

## Security issues
Do not file them as ordinary issues. See `SECURITY.md`.
