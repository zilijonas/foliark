// node --test tools/bench/check-budgets.test.mjs : the gate must fail on an injected regression and on a missing metric,
// and must not fail on one noisy run when another run of the same build is clean.
import assert from "node:assert/strict";
import { test } from "node:test";
import { SELECTORS, check, derive } from "./check-budgets.mjs";

const run = (scale = 1, drop = null, browser = "chromium 151") => ({
  browser, wasmGzipBytes: 2_100_000, assets: { wasmBytes: 5_500_000 },
  measurements: SELECTORS.filter((s) => s.id !== drop).map((s) => ({ op: s.op, input: s.input, ...(s.run && { run: s.run }), [s.field]: 100 * scale })),
});
const budgets = derive([run()]);
const status = (rows) => rows.filter((r) => r.status !== "ok" && r.status !== "skipped").map((r) => `${r.status}:${r.id}`);

test("a clean run passes", () => assert.deepEqual(status(check(budgets, [run()])), []));

test("an injected 2x slowdown fails every timing metric, an injected size growth fails the wasm budget", () => {
  const slow = status(check(budgets, [run(2)]));
  assert.ok(slow.length > 20 && slow.every((s) => s.startsWith("REGRESSION")));
  const fat = run(); fat.wasmGzipBytes = 2_300_000;
  assert.deepEqual(status(check(budgets, [fat])), ["REGRESSION:wasm gzip -9 bytes"]);
});

test("a budgeted metric that was not measured fails instead of passing", () =>
  assert.deepEqual(status(check(budgets, [run(1, "text 30p search")])), ["NOT MEASURED:text 30p search"]));

test("best of several runs isolates a noisy neighbour, but not a real slowdown", () => {
  assert.deepEqual(status(check(budgets, [run(3), run(1)])), []);
  assert.ok(status(check(budgets, [run(3), run(2.5)])).length > 20);
});

test("browsers without a fresh file are skipped, not passed silently", () => {
  const both = derive([run(), run(1, null, "webkit 26")]);
  const rows = check(both, [run()]);
  assert.ok(rows.some((r) => r.browser === "webkit" && r.status === "skipped"));
  assert.deepEqual(status(rows), []);
});
