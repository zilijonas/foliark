# Foliark

A headless PDF engine in Rust, compiled to WebAssembly, with a TypeScript SDK (`@foliark/sdk`). It extracts text, searches, renders pages,
edits paragraphs, redacts and exports sanitized PDFs, in the browser, on the user's device. The document is never uploaded.

Private and unreleased: version 0.1.0, UNLICENSED, nothing is published, local git only.

## Why

Its first consumer, HDDN (a private PDF redaction app), runs PyMuPDF inside Pyodide: a 30 MB download, a second of start-up and
a native export it cannot prove safe. Foliark replaces that with one small engine (about 2.1 MB gzip) whose native export is a
rebuilt, allowlisted file, checked by independent oracles, and which refuses what it cannot do safely with a typed error instead of
degrading quietly. Measured numbers and what is not established are in [docs/support-matrix.md](docs/support-matrix.md).

## Architecture

```
 your page / worker
        |  import { createFoliark } from "@foliark/sdk"        (or @foliark/sdk/inline inside your own worker)
        v
 packages/sdk   TypeScript client, supervisor (hang watchdog, hard reset), protocol guards, FoliarkError
        |  postMessage, validated envelope, buffers transferred
        v
 crates/foliark-wasm     wasm-bindgen facade, handle table, bounded stepping (cancel between steps)
        |
        v
 crates/foliark-engine   reader -> interpreter -> text | render | edit | redact | sanitize -> writer (+ raster export)
        |        \
        |         crates/foliark-text    font registry, shaping, line breaking, subsetting
        v
 vendor/hayro-*          forked PDF parser and interpreter (additive patches, patches/hayro/README.md)

 crates/foliark-model    DTOs, error codes, limits: defined once in Rust, generated to TypeScript (ts-rs)
```

The contract is [docs/spec/v1.md](docs/spec/v1.md) (frozen; changes need an ADR in [docs/adr](docs/adr)). Every operation completes or
rejects with a typed error; capability is never inferred from a preview.

## Use it

Build the tarball and install it into a project (see [packages/sdk/README.md](packages/sdk/README.md) for the API by example and
[docs/deployment.md](docs/deployment.md) for hosting, CSP and offline use):

```sh
scripts/build-wasm.sh && pnpm build:sdk && tests/package-consumers/pack.sh dist-pkg
npm install ./dist-pkg/foliark-sdk-0.1.0.tgz
```

The release candidate record, with checksums and the verification run, is [docs/release/candidate.md](docs/release/candidate.md).

## Develop

Prerequisites, the mandatory memory queue and every command (fmt, clippy, tests, wasm build, smoke, browser specs, package consumers,
benchmarks) are in [docs/development.md](docs/development.md). Evidence and decisions: [docs/evidence](docs/evidence),
[docs/performance](docs/performance), [docs/provenance](docs/provenance), [docs/integration/hddn-adapter.md](docs/integration/hddn-adapter.md).
Regression budgets: `node tools/bench/check-budgets.mjs FRESH.json` against `tools/bench/budgets.json`.
