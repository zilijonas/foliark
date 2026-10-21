# Vendored hayro fork

`vendor/hayro-syntax` and `vendor/hayro-interpret` are wired through `[patch.crates-io]` in the root `Cargo.toml`. The renderer `hayro` 0.8.0 stays on crates.io and picks up the patched interpret.

## Baseline

Unmodified upstream 0.8.0, copied from the crates.io source with only `.cargo-ok`, `.cargo_vcs_info.json`, `Cargo.lock` and `Cargo.toml.orig` removed.

| Crate | Version | .crate sha256 |
|---|---|---|
| hayro-syntax | 0.8.0 | `6cdc818d8d265dcb42081d488f7599df4f73fb79e9d347ecedcd3c23809d2026` |
| hayro-interpret | 0.8.0 | `ac9df074ff163a63864cec9b0bf640ccbff8abf4e44481df7369ce0ac172ffbe` |

## Rules

- Additive only: new methods, fields behind new types, new trait methods with default bodies. Never change existing behaviour of upstream API.
- Every change is upstreamable: self-contained, no Foliark types, documented like the surrounding code.
- One row per change below, in the same commit as the change. Planned set: spec `docs/spec/v1.md` section 13.1.

## Patches

| ID | Crate | Change | Files | Status |
|---|---|---|---|---|
| P1 | syntax | operator byte spans, per-content-stream identity | - | planned |
| P2 | interpret | `Device::begin_operator`, `begin_form/end_form` identity stack | - | planned |
| P3 | interpret | glyph raw char code, font ObjRef getters | - | planned |
| P4 | interpret | `InterpreterSettings::ocg_override` | - | planned |
| P5 | syntax, interpret | strict decode (Err instead of empty) | - | planned |
| P6 | syntax | `Pdf::was_repaired()` | - | planned |
