// Acceptance driver (NEW-59): runs every case through the INSTALLED npm candidate (the packed @foliark/sdk tarball in a temp
// project, real wasm, `@foliark/sdk/inline` entry) and writes the exported bytes plus a per-scenario record. It judges nothing:
// tests/integration/acceptance/judge.py does, with the independent oracles. Usage (from the temp project): node drive.mjs CASES.json OUTDIR
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createInlineFoliark } from "@foliark/sdk/inline";
import { FoliarkError } from "@foliark/sdk";

const [casesFile, outDir] = process.argv.slice(2);
const cases = JSON.parse(readFileSync(casesFile, "utf8"));
const wasm = readFileSync(new URL(import.meta.resolve("@foliark/sdk/wasm")));
const font = readFileSync(process.env.FONT_TTF);
const sha = (b) => createHash("sha256").update(b).digest("hex");
const fail = (e) => (e instanceof FoliarkError ? { outcome: "refused", code: e.code, details: e.details } : { outcome: "crash", message: String(e && e.stack || e) });

// Node's fetch cannot read file: URLs, and the packed default cmapsUrl is one: read the pack files explicitly.
const f = await createInlineFoliark({ wasm, fetchCMap: async (url) => new Uint8Array(readFileSync(new URL(url))) });
await f.registerFonts([{ bytes: font.slice(), family: "Liberation Sans", weight: 400, style: "normal" }]);
const open = (c, input) => f.open(new Uint8Array(input), c.password ? { password: c.password } : {});

/** Every occurrence of `secret` in the extracted text (hidden layers and invisible text included): [{page, start, end}]. */
async function locate(doc, pages, secret) {
  const hits = [];
  for (let p = 0; p < pages; p++) {
    const t = await doc.extractText(p);
    for (let i = t.text.indexOf(secret); i >= 0; i = t.text.indexOf(secret, i + 1)) hits.push({ page: p, start: i, end: i + secret.length });
  }
  return hits;
}
const bbox = (quads) => {
  const xs = quads.flat().map((p) => p.x), ys = quads.flat().map((p) => p.y);
  return { x: Math.min(...xs), y: Math.min(...ys), width: Math.max(...xs) - Math.min(...xs), height: Math.max(...ys) - Math.min(...ys) };
};
const toRect = (r) => ({ x: r[0], y: r[1], width: r[2] - r[0], height: r[3] - r[1] });
const textReds = (hits, tag) => hits.map((h, i) => ({ kind: "text", id: `${tag}${i}`, pageIndex: h.page, start: h.start, end: h.end }));

const hitRects = async (d, hits) => Promise.all(hits.map(async (h) => { const b = bbox((await d.quadsFor(h.page, h.start, h.end)).quads); return [h.page, b.x, b.y, b.x + b.width, b.y + b.height]; }));

async function finish(doc, mode, file, extra = {}) {
  const policy = mode === "raster" ? { raster: { acknowledgeLosses: true, scale: 2 } } : {};
  const plan = await doc.prepareExport({ mode, policy });
  const bytes = await doc.export(plan);
  writeFileSync(file, bytes);
  return { outcome: "ok", sha256: sha(bytes), bytes: bytes.length, mode, report: plan.report, ...extra };
}

async function scenario(c, input, name, fn) {
  const dir = join(outDir, c.id);
  mkdirSync(dir, { recursive: true });
  let doc;
  try {
    doc = await open(c, input);
    const n = (await doc.pages()).length;
    return await fn(doc, n, join(dir, `${name}.pdf`));
  } catch (e) {
    return fail(e);
  } finally {
    if (doc) await doc.close().catch(() => {});
  }
}

async function regionsFor(c, doc, n) {
  const reds = [];
  const add = (page, rect, tag) => reds.push({ kind: "region", id: `${tag}${reds.length}`, pageIndex: page, rect });
  for (const g of c.regions) if (g.kind.startsWith("sensitive") || g.kind === "region") add(g.page, toRect(g.rect), "d");
  for (const s of c.secrets.filter((x) => !c.survivors.includes(x))) for (const h of await locate(doc, n, s)) add(h.page, bbox((await doc.quadsFor(h.page, h.start, h.end)).quads), "s");
  return reds;
}

async function runCase(c) {
  const input = readFileSync(c.path);
  const rec = { input_sha256: sha(input), input_bytes: input.length, scenarios: {} };
  const S = rec.scenarios;
  S.sanitize = await scenario(c, input, "sanitize", (d, n, file) => finish(d, "native", file));
  S.region = await scenario(c, input, "region", async (d, n, file) => {
    const reds = await regionsFor(c, d, n);
    if (!reds.length) return { outcome: "n/a", reason: "no region and no located secret" };
    const r = await d.applyChanges({ baseRevision: 0, redactions: reds });
    if (!r.accepted) return { outcome: "refused", code: r.rejected[0]?.code, details: r.rejected[0]?.details, at: "applyChanges" };
    return finish(d, "native", file, { regions: reds.map((x) => [x.pageIndex, x.rect.x, x.rect.y, x.rect.x + x.rect.width, x.rect.y + x.rect.height]) });
  });
  S.text = await scenario(c, input, "text", async (d, n, file) => {
    if (c.survivors.length) return { outcome: "n/a", reason: "survivor case: region scenario only" };
    const hits = (await Promise.all(c.secrets.map((s) => locate(d, n, s)))).flat();
    if (!hits.length) return { outcome: "n/a", reason: c.secrets.length ? "secret not in extracted text" : "no text secret" };
    const r = await d.applyChanges({ baseRevision: 0, redactions: textReds(hits, "t") });
    if (!r.accepted) return { outcome: "refused", code: r.rejected[0]?.code, details: r.rejected[0]?.details, at: "applyChanges" };
    return finish(d, "native", file, { ranges: hits.length, regions: await hitRects(d, hits) });
  });
  S.raster = await scenario(c, input, "raster", async (d, n, file) => {
    const reds = await regionsFor(c, d, n);
    if (reds.length) {
      const r = await d.applyChanges({ baseRevision: 0, redactions: reds });
      if (!r.accepted) return { outcome: "refused", code: r.rejected[0]?.code, at: "applyChanges" };
    }
    return finish(d, "raster", file, { regions: reds.map((x) => [x.pageIndex, x.rect.x, x.rect.y, x.rect.x + x.rect.width, x.rect.y + x.rect.height]) });
  });
  S.raster_sanitize = await scenario(c, input, "raster_sanitize", (d, n, file) => finish(d, "raster", file));
  S.edit = await scenario(c, input, "edit", async (d, n, file) => {
    let secret, hits = [];
    for (const s of c.secrets) if (!hits.length && s.length > 3) { secret = s; hits = await locate(d, n, s); }
    if (!hits.length) return { outcome: "n/a", reason: "no located text secret to retype" };
    const t = await d.extractText(0);
    const w = t.words.find((x) => x.text.startsWith("KEEP_PUBLIC") && !x.invisible);
    if (!w) return { outcome: "n/a", reason: "no public word on page 0 to retype over" };
    const line = t.words.filter((x) => x.blockIndex === w.blockIndex && x.lineIndex === w.lineIndex && !x.invisible);
    const x0 = Math.min(...line.map((x) => x.box.x)), x1 = Math.max(...line.map((x) => x.box.x + x.box.width));
    const y0 = Math.min(...line.map((x) => x.box.y)), y1 = Math.max(...line.map((x) => x.box.y + x.box.height));
    const rect = { x: x0 - 1, y: y0 - 1, width: x1 - x0 + 2, height: y1 - y0 + 2 };
    const e = await d.applyChanges({ baseRevision: 0, textEdits: [{ id: "e0", pageIndex: 0, rects: [rect], text: secret }] });
    if (!e.accepted) return { outcome: "refused", code: e.rejected[0]?.code, at: "edit" };
    const after = await locate(d, n, secret);
    const r = await d.applyChanges({ baseRevision: e.revision, redactions: textReds(after, "x") });
    if (!r.accepted) return { outcome: "refused", code: r.rejected[0]?.code, at: "redact-after-edit" };
    return finish(d, "native", file, { edit_rect: [0, rect.x, rect.y, rect.x + rect.width, rect.y + rect.height], retyped: secret, regions: await hitRects(d, after) });
  });
  return rec;
}

// Cancellation: abort an export / a redaction apply after k event-loop turns, then re-export. The exported bytes must equal a clean run
// (deterministic, no stale bytes of the cancelled attempt) and an aborted apply must be atomic (all redactions or none).
async function cancelRuns(c) {
  const input = readFileSync(c.path);
  const dir = join(outDir, c.id);
  mkdirSync(dir, { recursive: true });
  const reds = c.regions.map((g, i) => ({ kind: "region", id: `c${i}`, pageIndex: g.page, rect: toRect(g.rect) }));
  const prep = async () => {
    const d = await open(c, input);
    return d;
  };
  const clean = await prep();
  await clean.applyChanges({ baseRevision: 0, redactions: reds });
  const baseline = await clean.export(await clean.prepareExport({ mode: "native" }));
  await clean.close();
  const rc = await prep();
  await rc.applyChanges({ baseRevision: 0, redactions: reds });
  const rasterBaseline = await rc.export(await rc.prepareExport({ mode: "raster", policy: { raster: { acknowledgeLosses: true, scale: 2 } } }));
  await rc.close();
  const out = { baseline_sha256: sha(baseline), baseline_raster_sha256: sha(rasterBaseline), attempts: [] };
  const hop = async (k) => { for (let i = 0; i < (k === "sync" ? 0 : k); i++) await new Promise((r) => setImmediate(r)); };
  for (const phase of ["apply", "prepare", "raster"]) {
    for (const k of ["pre", "sync", 0, 1, 2, 4, 8, 16, 32]) {
      const d = await prep();
      const ac = new AbortController();
      if (k === "pre") ac.abort();
      const a = { phase, k };
      // two cheap ops queued ahead of the target, so an abort can win while the target waits (an op already running is not interrupted mid-slice)
      const blockers = [d.renderPage(0, { scale: 3 }), d.extractText(0)];
      try {
        if (phase === "apply") {
          const p = d.applyChanges({ baseRevision: 0, redactions: reds }, { signal: ac.signal });
          (k === "sync" ? Promise.resolve(ac.abort()) : k === "pre" ? 0 : hop(k).then(() => ac.abort()));
          try { const r = await p; a.first = r.accepted ? "accepted" : "rejected"; } catch (e) { a.first = e.code || "crash"; }
          // atomicity: revision says whether the redactions are in; the export must agree
          const plan = await d.prepareExport({ mode: "native" });
          const bytes = await d.export(plan);
          const file = join(dir, `cancel-${phase}-${k}.pdf`);
          writeFileSync(file, bytes);
          a.revision = plan.revision; a.sha256 = sha(bytes); a.file = file;
          if (a.first !== "accepted" && plan.revision === 0) {
            // not applied: re-apply and re-export must equal the baseline
            await d.applyChanges({ baseRevision: 0, redactions: reds });
            const again = await d.export(await d.prepareExport({ mode: "native" }));
            a.reapplied_sha256 = sha(again);
          }
        } else {
          await d.applyChanges({ baseRevision: 0, redactions: reds });
          blockers.push(d.renderPage(0, { scale: 3 }), d.extractText(0));
          const p = d.prepareExport({ mode: phase === "raster" ? "raster" : "native", policy: { raster: { acknowledgeLosses: true, scale: 2 } }, signal: ac.signal });
          (k === "sync" ? Promise.resolve(ac.abort()) : k === "pre" ? 0 : hop(k).then(() => ac.abort()));
          try { await p; a.first = "completed"; } catch (e) { a.first = e.code || "crash"; }
          const again = await d.export(await d.prepareExport({ mode: phase === "raster" ? "raster" : "native", policy: { raster: { acknowledgeLosses: true, scale: 2 } } }));
          const file = join(dir, `cancel-${phase}-${k}.pdf`);
          writeFileSync(file, again);
          a.sha256 = sha(again); a.file = file;
        }
      } catch (e) { a.error = fail(e); }
      await Promise.allSettled(blockers);
      await d.close().catch(() => {});
      out.attempts.push(a);
    }
  }
  return out;
}

const results = {};
for (const c of cases) results[c.id] = await runCase(c);
const cc = cases.find((c) => c.id === "multipage-12");
const cancel = cc ? await cancelRuns(cc) : null;
await f.dispose();
writeFileSync(join(outDir, "results.json"), JSON.stringify({ results, cancel }, null, 1));
console.log(`drove ${cases.length} cases`);
