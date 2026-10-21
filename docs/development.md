# Foliark development

Contract: [docs/spec/v1.md](spec/v1.md) (frozen; changes need an ADR in `docs/adr/`). Rules for agents: `CLAUDE.md`. Board: Workbench project `NEW` (`AGENTS.md`).

## Prerequisites
- Rust via `rustup`; `rust-toolchain.toml` pins 1.99.1 with `wasm32-unknown-unknown`, clippy, rustfmt. `source ~/.cargo/env` if `cargo` is missing.
- `wasm-bindgen-cli` 0.2.129, exactly the `wasm-bindgen` crate pin. Prebuilt aarch64-apple-darwin tarball from github.com/rustwasm/wasm-bindgen releases into `~/.cargo/bin`, or `cargo install wasm-bindgen-cli --version 0.2.129 --locked`. `scripts/build-wasm.sh` refuses any other version.
- Node 26 and pnpm 11. Playwright (npm devDependency, 1.62.1) uses browsers already in `~/Library/Caches/ms-playwright`; otherwise `pnpm exec playwright install chromium webkit`.
- No Python or C/C++ PDF engine in the shipped graph. PyMuPDF is allowed only as a dev oracle under `tests/oracles`.

## Memory queue (mandatory)
Every `cargo build/test/clippy`, `wasm-bindgen` and browser run goes through the queue; `$OWNER` is your session id:
```sh
~/ai/bin/memory-queue run --owner $OWNER --label 'foliark <what>' --need-gb 4 --restartable --wait 1800 -- <cmd>
```
Use `--need-gb 2` for browser-only runs (smoke). Exit 75 = deferred: wait and retry, not a failure. Run long jobs in the background and do not poll. `cargo fmt --check` and file edits need no reservation.

## Commands
Run from the repo root, each wrapped as above.

| Goal | Command |
|---|---|
| Format check | `cargo fmt --check` |
| Lint (vendor excluded) | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| All Rust tests | `cargo test --workspace --locked` |
| One crate | `cargo test -p foliark-engine` (also `-p foliark-model`, `-p foliark-wasm`) |
| Wasm build + JS glue | `scripts/build-wasm.sh` (output `packages/sdk/wasm/`, gitignored; prints raw and gzip size) |
| Wasm compile check only | `cargo build --locked --target wasm32-unknown-unknown -p foliark-wasm` |
| SDK build | `pnpm install && pnpm --filter @foliark/sdk build` (needs the wasm glue first for its types) |
| SDK tests | `pnpm --filter @foliark/sdk test` (vitest; protocol only: `-- protocol.spec.ts`) |
| Rust/TS parity | `scripts/check-ts-parity.sh` (regenerates ts-rs into a temp dir, diffs `packages/sdk/src/generated`; non-zero on drift) |
| Browser smoke | `pnpm smoke chromium` and `pnpm smoke webkit` (needs wasm and SDK built) |
| Regenerate smoke fixture | `cargo run -q -p foliark-engine --example smoke_pdf > tests/smoke/one-page.pdf` |
| Oracles | placeholder: `tests/oracles` is owned by the independent-checker tasks (NEW-7, NEW-20, NEW-21); no command yet |

Order for a full local check: fmt, clippy, test, `scripts/build-wasm.sh`, SDK build, smoke. CI (`.github/workflows/ci.yml`) mirrors it and has never run (local git only).

ts-rs writes TypeScript DTOs to `packages/sdk/src/generated/` during `cargo test`; commit the result, never hand-edit it.

## Dependencies and the hayro fork
Pins are exact (`=`) in the root `Cargo.toml`; `Cargo.lock` is committed, always pass `--locked`. `vendor/hayro-syntax` and `vendor/hayro-interpret` are patched in via `[patch.crates-io]` and are not workspace members (not linted). Every change to them is additive, upstreamable and gets a row in `patches/hayro/README.md` (spec 13.1 lists P1 to P6).

## Receipts and handoff
- Attach one Markdown receipt per task via `attach_artifact` with inline `content` (never `sourcePath`), at most about 1500 chars: commit hash, files touched, exact commands with exit codes, counts, limitations, next step. Check each Done-when line explicitly; never claim an unrun check.
- Set the task to `review` after attaching; the lead approves. Do not leave a task in `doing`: hand it back as `todo` with a note or set `blocked` with a reason.
- Local git only: commit on the given branch, never push or add remotes. Commit messages are conventional and end with the Co-Authored-By line.
- Findings that change a later task go in `update_task` plus `add_note`.

## Module to owner task (spec 14)
| Module | Owner tasks |
|---|---|
| `foliark-model` (DTOs, codes, Limits, geometry) | NEW-15, NEW-16 |
| `foliark-engine::reader` | NEW-10, NEW-11, NEW-13 |
| `::interp` (provenance, clusters, OC state) | NEW-12, NEW-14, NEW-43 |
| `::text` (extract, hit test, select, search) | NEW-17, NEW-18, NEW-19 |
| `::render` | NEW-25 |
| `::edit` (revisions, ChangeSet, paragraphs) | NEW-28, NEW-50, NEW-51 |
| `::redact` | NEW-32, NEW-33, NEW-35, NEW-39 |
| `::sanitize` | NEW-41, NEW-42, NEW-44, NEW-45 |
| `::write` (serializer, raster writer, verify) | NEW-23, NEW-24, NEW-46, NEW-56 |
| `foliark-text` (fonts, shaping, layout) | NEW-48, NEW-49 |
| `foliark-wasm` | NEW-27, NEW-29 |
| `packages/sdk` | NEW-15 (protocol), NEW-27, NEW-29, NEW-30 |
| `tests/smoke`, build, CI | NEW-4 (this skeleton) |
| `tests/oracles`, fuzz | NEW-7, NEW-57, NEW-58 |
