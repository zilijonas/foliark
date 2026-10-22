# Security policy

## Supported versions
| Version | Status | Security fixes |
|---|---|---|
| 0.1.x | private candidate, unreleased, not published anywhere | yes, on the development branch |
| anything else | none exists | no |

After the first public release this table lists the supported release lines and their end dates. Pre-1.0, only the latest 0.x line is supported.

## What counts as a privacy defect
Foliark's promise is that content which is redacted or sanitized is gone from the exported file. Therefore:
- **Critical:** any recoverable redacted or sanitized content in an export (text, image samples, metadata, annotations, form values, attachments, hidden layers, old revisions, anything an extractor, a byte search or a decoder can read back), or an export that reports `SAFE` while content remains.
- **High:** content outside the declared regions altered or lost without a typed disclosure; a refusal that should have happened (`SIGNED_INPUT`, `RENDER_INCOMPLETE`, `CONSENT_REQUIRED`) not happening; network requests from the SDK after asset load.
- **High (availability):** a crafted PDF that exhausts memory or never finishes past the configured `Limits` or watchdog.
- **Medium and below:** wrong render or extraction without privacy impact. These are ordinary bugs; file them normally.

Unsupported input listed in `docs/support-matrix.md` as refused or not proven is documented behaviour, not a vulnerability, unless the export claims success.

## Reporting
Private channel: to be set by the owner (NEW-73). Until it is set there is no public reporting address; do not post details in a public place.
When the channel exists, send the PDF (or a minimized one), the SDK version, the call sequence and what was recovered.

## Triage and regression policy
1. Reproduce, then minimize the PDF to the smallest fixture that still shows the defect.
2. Add the fixture and a failing regression test (and a negative control if an oracle missed it) before the fix.
3. Fix, then re-run the acceptance suite (`tests/integration/acceptance/run.sh`), the strict-native and fuzz-smoke suites, and the full verification list in `docs/release/process.md`. A fix is not done until the acceptance counts are recorded and have no blockers.
4. Record the fixture in the corpus so it is checked on every release.

## Dependency and fork updates
- The wasm contains 94 production crates (`docs/provenance/dependencies.json`, `packages/sdk/THIRD_PARTY_NOTICES.md`, SBOM). The npm package has no runtime dependencies.
- `cargo deny check licenses bans sources` (`deny.toml`) must pass; it does not check advisories yet. Until an advisory database is wired into CI, check RustSec manually before each release and after any report.
- Vendored hayro fork (`vendor/hayro-syntax`, `vendor/hayro-interpret`, rows P1 to P17 in `patches/hayro/README.md`): pins are exact. To rebase onto a new upstream release, copy the new crates.io source over a clean tree, re-apply each patch row in order (each is additive and upstreamable), update the baseline sha256 table, run the full verification, and regenerate notices, inventory and SBOM. Prefer upstreaming rows so the fork shrinks.
- A security fix in a dependency ships as a patch release of the SDK with a changelog entry.

## Disclosure timeline
Targets, to be confirmed by the owner with the reporting channel (NEW-73): acknowledge within 3 working days, assess within 10, fix a critical privacy defect in a patch release within 30 days, then publish an advisory and a `CHANGELOG.md` entry naming the fixed versions. Reporters are credited if they wish.
