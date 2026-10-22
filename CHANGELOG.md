# Changelog

All notable changes to `@foliark/sdk`. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: `docs/release/process.md` (semver; 0.x may break with an entry here).

## [Unreleased]

## [0.1.0] - unreleased (private candidate)

Not published. Candidate tarball and checksums: `docs/release/candidate.md`. The licence is a placeholder until NEW-73.

### Added
- Headless Rust/WASM PDF SDK that runs in a worker, with no network after the files are cached: open (RC4-40/128 and AES-128/256 encrypted input with the password), extract text, search, hit test, select, render.
- Paragraph edit for horizontal, unrotated, left-to-right text, with stale-revision detection.
- Native export: edit, flatten, redact, sanitize, rewrite, verify. Independent oracles found no leftover content and no collateral damage on the cited corpus (`docs/evidence/strict-native.md`).
- Raster export (image-only, redactions burned in), behind explicit `acknowledgeLosses` consent.
- Typed refusals and warnings instead of silent fallback (`MALFORMED_PDF`, `ENCRYPTION_UNSUPPORTED`, `SIGNED_INPUT`, `LIMIT_EXCEEDED`, `RENDER_INCOMPLETE`, `CMAP_MISSING` and others), cancellation, worker watchdog and recovery.
- Notices (`THIRD_PARTY_NOTICES.md`, `NOTICE`, `LICENSE_FOXIT`), support matrix in the package, CycloneDX SBOM next to the tarball, release workflow and local dry run.

### Known limits
- Browsers: Chromium (desktop, macOS arm64) supported with budgets; WebKit engine supported, not as Safari; Firefox functional only. Real Safari, iPhone, iPad, Android, Windows and Linux: not established (NEW-64, device access NEW-22). No minimum browser version is promised.
- Predefined CMaps (legacy CJK encodings) are unsupported: `cmapsUrl` is accepted and ignored, render warns `CMAP_MISSING`.
- Paragraph edit does not cover CFF/Type1 embedded faces, RTL, CJK, vertical text or auto-fit.
- Encrypted raster fidelity, abort while an operation runs through the packed SDK, and files unlike the synthetic corpus are not proven.
- Budgets were frozen on one machine (Apple M4); reproducibility across machines is not shown.
- Foxit font terms beyond the PDFium notice are unverified; cargo-deny runs without an advisory check; no legal clearance of the notices.
- See `docs/support-matrix.md` for every row and its evidence.

[Unreleased]: docs/release/process.md
[0.1.0]: docs/release/candidate.md
