# Foliark — agent rules

Headless Rust/WASM PDF SDK. First consumer: HDDN (`~/ai/repos/hddn`), replacing Pyodide + PyMuPDF. Board: Workbench project `NEW` (see AGENTS.md). The lead agent assigns tasks; you execute only the task(s) you were given.

## Hard rules
- Local git only. Never push, never add remotes, never publish packages.
- No Codex. No C/C++ PDF engines (PDFium, MuPDF, Poppler, qpdf) in the shipped graph; they may be used only as dev-only oracles in `tests/oracles` (PyMuPDF via `python3 -c "import fitz"` is available).
- Coding follows Ponytail (`~/.claude/skills/ponytail/SKILL.md`): smallest complete change, reuse before adding, no speculative abstraction. Keep validation, error handling and security.
- Tests prove behavior that could break. No tests that mirror the implementation, no trivial getter tests.
- Every `cargo build/test/clippy`, `wasm-bindgen`, browser or Playwright run goes through the memory queue:
  `~/ai/bin/memory-queue run --owner 807494d7-f0ff-4dfb-a3f4-ac700f52c9ef --label 'foliark <what>' --need-gb 4 --restartable --wait 1800 -- <cmd>`
  Exit 75 means deferred: wait and retry, do not treat as failure. Long runs go in the background; do not poll.
  Never nest reservations: scripts that already call memory-queue themselves (`tools/fuzz-smoke/run.sh`) run unwrapped; wrapping them deadlocks the queue.
- `source ~/.cargo/env` if `cargo` is not on PATH.
- Workbench artifacts: pass Markdown inline via `content`. Never pass `sourcePath`.
- Do not modify `~/ai/repos/hddn` unless your task is an HDDN integration task; then work only in the worktree you are given.
- Commit your work on the branch you are given with a normal conventional-commit message ending with
  `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` (or your actual model).

## Receipts
Attach one Markdown receipt per task (<= ~1500 chars): commit hash, files touched, exact commands + exit codes, case counts, limitations, next step. Check every Done-when line explicitly; never claim a check you did not run. If blocked, say exactly what is missing.

## Layout (planned)
- `crates/foliark-model` — ids, geometry, DTOs (serde, ts-rs)
- `crates/foliark-engine` — reader, interpreter, render, text/search, transactions, redaction, sanitizer, writer
- `crates/foliark-text` — font registry, paragraph shaping/layout
- `crates/foliark-wasm` — wasm-bindgen facade, bounded stepping
- `packages/sdk` — TS worker protocol, supervisor, public ESM API
- `tests/fixtures`, `tests/oracles`, `tests/integration`, `fuzz`, `examples/vanilla`, `tools/bench`, `docs`
