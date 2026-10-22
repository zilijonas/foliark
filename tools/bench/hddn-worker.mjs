// HDDN baseline: drives the REAL built redactor worker (HDDN@2fe0f9a dist) from a Playwright page.
// Usage (standalone): node tools/bench/hddn-worker.mjs [--smoke] [--browser chromium|webkit]
import { mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import playwright from "playwright";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { BENCH, EDIT_LINES, FIXTURES, EXTENDED_FIXTURES, HDDN_DIST, OUT_DIR, loadFixtures, serve, stat1, sumBytes, treeRssBytes } from "./lib.mjs";

const SCALE = 1.35;
const ARGS = ["--enable-features=SharedArrayBuffer"];

// extended: the full op set of docs/performance/comparison.json (paragraph prepare, DETECT, N-box native + flattened export, memory).
// lines: {fixtureId: [{x, y, width, height, text, pageW, pageH}]} (SDK display-space lines) so both engines get identical regions.
// only/scales (NEW-81): run just these fixtures, previews only, at each scale.
export async function runHddn({ smoke = false, browserName = "chromium", extended = false, lines = {}, only, scales = [SCALE] } = {}) {
  const N = smoke ? { cold: 1, warmInit: 2, op: 2, heavy: 2 } : { cold: 3, warmInit: 5, op: 5, heavy: 3 };
  const workerFile = (await readdir(join(HDDN_DIST, "_astro"))).find((f) => /^redactor\.worker-.*\.js$/.test(f));
  if (!workerFile) throw new Error("HDDN dist missing; build with `pnpm exec astro build` in .worktrees/hddn-2fe0f9a");
  // Worker behind the memory probe: same origin, so HDDN keeps resolving its assets against "/".
  await mkdir(join(BENCH, ".cache"), { recursive: true });
  await writeFile(join(BENCH, ".cache/hddn-wrap.js"), `import "/__bench/memprobe.js";\nimport "/_astro/${workerFile}";\n`);
  const fixtures = await loadFixtures(only ? only : smoke ? FIXTURES.filter((f) => f[0] === "native-small" || f[0] === "native-30p") : extended ? EXTENDED_FIXTURES : FIXTURES);
  const srv = await serve([{ prefix: "/__bench/", dir: BENCH }, { prefix: "/", dir: HDDN_DIST }]);
  const url = `${srv.origin}/__bench/hddn-page.html`;
  const measurements = [], unavailable = [], assets = {}, workerLog = [];
  const add = (op, input, run, samples, extra = {}) => measurements.push({ op, input, run, ...stat1(samples), ...extra });
  const launch = async (dir) => {
    const ctx = await playwright[browserName].launchPersistentContext(dir, { args: browserName === "chromium" ? ARGS : [] });
    return ctx;
  };
  const open = async (ctx) => {
    const page = await ctx.newPage();
    page.on("pageerror", (e) => console.error("pageerror:", e.message));
    // worker console output is the only place HDDN's Python tracebacks show up (the protocol reports "errors.unknown")
    const logWorker = (w) => w.on("console", (m) => { if (/error|traceback|exception|memory/i.test(m.text())) workerLog.push(m.text().slice(0, 400)); });
    page.on("worker", logWorker); page.on("console", (m) => { if (/error|traceback|exception|memory/i.test(m.text())) workerLog.push(m.text().slice(0, 400)); });
    await page.goto(url);
    await page.evaluate((w) => startWorker(w), "/__bench/.cache/hddn-wrap.js");
    return page;
  };
  const init = (page) => page.evaluate(async () => (await W.req("INIT", { baseUrl: "/" })).ms);
  let importSeq = 0;
  // allowIncompleteOcr: a scanned page is in the OCR lane and COMMIT_IMPORT refuses it until OCR ran (NEW-81: this refusal was the cause of the GET_PAGE_PREVIEW "errors.unknown" on image-heavy files; OCR itself is not measured here).
  // LOAD_PDF stages the import under an importId; COMMIT_IMPORT (timed separately) makes it the active document, as the app does.
  const loadPdf = (page, f, nSpans = 1) => page.evaluate(async ({ b64, name, size, importId, nSpans }) => {
    const bin = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const r = await W.req("LOAD_PDF", { importId, file: bin.buffer, name, size, mimeType: "application/pdf" }, [bin.buffer]);
    if (r.type === "PDF_LOADED") { const c = await W.req("COMMIT_IMPORT", { importId, allowIncompleteOcr: true }); r.commitMs = c.ms; r.commitType = c.type; }
    return { ms: r.ms, commitMs: r.commitMs, commitType: r.commitType, type: r.type, spans: r.payload.spans?.slice(0, nSpans), pages: r.payload.pages?.length, err: r.type === "ERROR" ? r.payload : undefined };
  }, { b64: f.buf.toString("base64"), name: `${f.id}.pdf`, size: f.bytes, importId: `bench-${++importSeq}`, nSpans });
  const extendedOps = async (page, f, first) => {
    const common = { pages: first.pages };
    const norm = (l) => ({ x: l.x / l.pageW, y: l.y / l.pageH, width: l.width / l.pageW, height: l.height / l.pageH });
    const ls = lines[f.id]; if (!ls?.length) { unavailable.push({ op: "extended ops", input: f.id, reason: "no SDK lines to share" }); return; }
    const boxes = ls.map(norm), span = { box: boxes[0], text: ls[0].text };
    const N2 = Math.min(N.op, 3);
    const ph = [];
    for (let i = 0; i < N2; i++) {
      const r = await page.evaluate(async (box) => { const x = await W.req("PREPARE_PARAGRAPH_EDIT", { pageIndex: 0, boxes: [box] }); return { ms: x.ms, type: x.type, status: x.payload?.status }; }, span.box);
      if (r.type !== "PARAGRAPH_EDIT_PREPARED" || r.status !== "ok") { unavailable.push({ op: "PREPARE_PARAGRAPH_EDIT", input: f.id, reason: `${r.type} ${r.status}` }); break; }
      ph.push(r.ms);
    }
    if (ph.length) add("PREPARE_PARAGRAPH_EDIT", f.id, "warm", ph, common);
    const ed = [];
    for (let i = 0; i < N2; i++) {
      const r = await page.evaluate(async ({ box, text }) => { const x = await W.req("SET_TEXT_EDITS", { edits: [{ id: "e1", pageIndex: 0, boxes: [box], replacementText: text }] }); return { ms: x.ms, type: x.type, acc: x.payload.accepted, rej: x.payload.rejectedEdits }; }, { box: span.box, text: span.text.slice(0, -1) });
      if (r.type !== "TEXT_EDITS_SET" || !r.acc) { unavailable.push({ op: "SET_TEXT_EDITS (same line + text as SDK)", input: f.id, reason: `${r.type} ${JSON.stringify(r.rej)}`.slice(0, 200) }); break; }
      ed.push(r.ms);
    }
    await page.evaluate(() => W.req("SET_TEXT_EDITS", { edits: [] }));
    if (ed.length) add("SET_TEXT_EDITS (same line + text as SDK)", f.id, "warm", ed, common);
    const det = []; let items = 0;
    for (let i = 0; i < N2; i++) { const r = await page.evaluate(async () => { const x = await W.req("DETECT", { rules: { keywords: ["alpha"] } }); return { ms: x.ms, type: x.type, n: x.payload.items?.length }; }); if (r.type !== "DETECTIONS") { unavailable.push({ op: "DETECT", input: f.id, reason: r.type }); break; } det.push(r.ms); items = r.n; }
    if (det.length) add("DETECT keywords ['alpha'] (rules + names + signatures, NOT a plain search)", f.id, "warm", det, { ...common, groups: items });
    await mkdir(OUT_DIR, { recursive: true });
    for (const [mode, label] of [["native", "APPLY_REDACTIONS native (N boxes)"], ["flattened", "APPLY_REDACTIONS flattened @2 (N boxes)"]]) {
      const ms = []; let out;
      for (let i = 0; i < N2; i++) {
        const r = await page.evaluate(async ({ boxes, mode }) => { const x = await W.req("APPLY_REDACTIONS", { redactions: { detections: [], textEdits: [], mode, manualRedactions: boxes.map((box, i) => ({ id: `m${i}`, pageIndex: 0, mode: "box", box, status: "confirmed" })) } }); const f = x.payload.file; let b64; if (f) { const u = new Uint8Array(f); let s = ""; for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode(...u.subarray(i, i + 0x8000)); b64 = btoa(s); } return { ms: x.ms, type: x.type, n: f?.byteLength, b64, err: x.type === "ERROR" ? x.payload : undefined }; }, { boxes, mode });
        if (r.type !== "REDACTED_FILE") { unavailable.push({ op: label, input: f.id, reason: `${r.type} ${JSON.stringify(r.err)}`.slice(0, 250) }); break; }
        ms.push(r.ms); out ??= r;
      }
      if (!out) continue;
      await writeFile(join(OUT_DIR, `hddn-${f.id}-${mode}.pdf`), Buffer.from(out.b64, "base64"));
      add(label, f.id, "warm", ms, { ...common, regions: boxes.length, outBytes: out.n });
    }
    measurements.push({ op: "memory after ops", input: f.id, run: "warm", rssBytes: treeRssBytes(), ...(await page.evaluate(() => wasmMem())) });
      };
  let browserVersion = "?";
  let heapInit = null;
  const tmpDirs = [];
  try {
    // cold: fresh profile (empty HTTP cache) per run; INIT then first LOAD_PDF in a fresh worker
    const coldInit = [], coldLoad = [];
    let warmCtx, warmDir;
    for (let i = 0; i < N.cold; i++) {
      const dir = await mkdtemp(join(tmpdir(), "hddn-bench-")); tmpDirs.push(dir);
      const ctx = await launch(dir); browserVersion = ctx.browser()?.version() ?? browserVersion;
      srv.bytes.clear();
      const page = await open(ctx);
      coldInit.push(await init(page));
      if (i === 0) { Object.assign(assets, sumBytes(srv.bytes)); assets.files = Object.fromEntries([...srv.bytes].filter(([p]) => p.startsWith("/pyodide/") || p.includes("redactor.worker")).map(([p, n]) => [p, n])); }
      const l = await loadPdf(page, fixtures[0]); coldLoad.push(l.ms);
      if (i === 0) { heapInit = treeRssBytes(); }
      if (i === N.cold - 1) { warmCtx = ctx; warmDir = dir; await page.close(); } else await ctx.close();
    }
    add("INIT", "-", "cold", coldInit, { bytes: assets.runtime + assets.app });
    add("LOAD_PDF", fixtures[0].id, "cold", coldLoad);

    // warm INIT: new page+worker, HTTP cache populated by the cold run
    const warmInit = [];
    srv.bytes.clear();
    for (let i = 0; i < N.warmInit; i++) { const p = await open(warmCtx); warmInit.push(await init(p)); await p.close(); }
    add("INIT", "-", "warm", warmInit, { bytes: Object.values(sumBytes(srv.bytes)).reduce((a, b) => a + b, 0) });
    if (!srv.bytes.size) assets.warmRefetchBytes = 0; else assets.warmRefetchBytes = Object.values(sumBytes(srv.bytes)).reduce((a, b) => a + b, 0);

    // warm ops: one worker, per fixture
    const page = await open(warmCtx); await init(page);
    for (const f of fixtures) {
      const loads = [], commits = [];
      let first;
      for (let i = 0; i < N.op; i++) { const l = await loadPdf(page, f, EDIT_LINES); if (l.err) { unavailable.push({ op: "LOAD_PDF", input: f.id, reason: `worker ERROR ${JSON.stringify(l.err)}` }); break; } loads.push(l.ms); commits.push(l.commitMs); first ??= l; }
      if (!loads.length) continue;
      add("LOAD_PDF", f.id, "warm", loads, { pages: first.pages });
      add("COMMIT_IMPORT", f.id, "warm", commits);
      if (f.id === "native-30p") measurements.push({ op: "rss-after-LOAD_PDF", input: f.id, run: "warm", rssBytes: treeRssBytes() });
      // previews, every page, N rounds, at each scale
      for (const sc of scales) {
        const perPage = Array.from({ length: first.pages }, () => []);
        for (let r = 0; r < N.op; r++) for (let p = 0; p < first.pages; p++) {
          const res = await page.evaluate(async ({ p, s }) => { const x = await W.req("GET_PAGE_PREVIEW", { pageIndex: p, scale: s }); return { ms: x.ms, type: x.type, len: x.payload.bytes?.byteLength, err: x.type === "ERROR" ? JSON.stringify(x.payload).slice(0, 300) : undefined }; }, { p, s: sc });
          if (res.type !== "PAGE_PREVIEW") { unavailable.push({ op: "GET_PAGE_PREVIEW", input: f.id, reason: `${res.type} ${res.err ?? ""} | worker log: ${workerLog.splice(0).slice(-3).join(" // ")}`.trim() }); break; }
          perPage[p].push(res.ms);
        }
        if (perPage.every((a) => a.length)) add(`GET_PAGE_PREVIEW@${sc}`, f.id, "warm", perPage.flat(), { pages: first.pages, perPageMedianMs: perPage.map((a) => stat1(a).ms) });
      }
      if (only) continue;
      // native redaction with one manual box over the first span
      const span = first.spans?.[0];
      if (extended && span && (f.id === "native-30p" || f.id === "native-small")) await extendedOps(page, f, first);
      if (!span) { unavailable.push({ op: "APPLY_REDACTIONS", input: f.id, reason: "no text span to box" }); }
      else {
        const red = [];
        for (let i = 0; i < N.heavy; i++) {
          const res = await page.evaluate(async (box) => { const x = await W.req("APPLY_REDACTIONS", { redactions: { detections: [], textEdits: [], mode: "native", manualRedactions: [{ id: "m1", pageIndex: 0, mode: "box", box, status: "confirmed" }] } }); return { ms: x.ms, type: x.type, out: x.payload.file?.byteLength, err: x.type === "ERROR" ? x.payload : undefined }; }, span.box);
          if (res.type !== "REDACTED_FILE") { unavailable.push({ op: "APPLY_REDACTIONS native", input: f.id, reason: `${res.type} ${JSON.stringify(res.err)}` }); break; }
          red.push(res.ms); first.out = res.out;
        }
        if (red.length) add("APPLY_REDACTIONS native (1 manual box)", f.id, "warm", red, { outBytes: first.out });
        if (f.role.includes("native text") && span.text.length > 1) {
          const ed = [];
          for (let i = 0; i < N.heavy; i++) {
            const res = await page.evaluate(async ({ box, text }) => { const x = await W.req("SET_TEXT_EDITS", { edits: [{ id: "e1", pageIndex: 0, boxes: [box], replacementText: text.slice(0, -1) }] }); return { ms: x.ms, type: x.type, acc: x.payload.accepted, rej: x.payload.rejectedEdits, err: x.type === "ERROR" ? x.payload : undefined }; }, { box: span.box, text: span.text });
            if (res.type !== "TEXT_EDITS_SET" || !res.acc) { unavailable.push({ op: "SET_TEXT_EDITS", input: f.id, reason: `${res.type} accepted=${res.acc} ${JSON.stringify(res.rej ?? res.err)}`.slice(0, 200) }); break; }
            ed.push(res.ms);
          }
          if (ed.length) add("SET_TEXT_EDITS (1 span)", f.id, "warm", ed);
          await page.evaluate(() => W.req("SET_TEXT_EDITS", { edits: [] }));
        }
      }
    }
    measurements.push({ op: "rss-after-INIT+first-LOAD_PDF", input: fixtures[0].id, run: "cold", rssBytes: heapInit });
    unavailable.push({ op: "JS heap (worker)", reason: "performance.measureUserAgentSpecificMemory throws SecurityError 'not available' in headless Chromium even with COOP/COEP (crossOriginIsolated page) and is absent in WebKit; process-tree RSS recorded instead (whole browser, not JS heap)" });
    await warmCtx.close();
  } finally {
    srv.close();
    for (const d of tmpDirs) await rm(d, { recursive: true, force: true });
  }
  unavailable.push({ op: "optional fonts / OCR (Tesseract) bytes", reason: "not fetched by these ops (fonts load lazily for edits with fallback; OCR only for scanned pages); see assets.fonts/ocr counters" });
  const workerSha = createHash("sha256").update(await readFile(join(HDDN_DIST, "_astro", workerFile))).digest("hex");
  assets.workerSha256 = workerSha; assets.fontsBytesWarmRun = sumBytes(srv.bytes).fonts;
  return { engine: "hddn@2fe0f9a", browser: `${browserName} ${browserVersion}`, inputs: fixtures.map(({ id, sha256, pages, bytes, origin }) => ({ id, sha256, pages, bytes, origin })), measurements, unavailable, assets };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const bi = process.argv.indexOf("--browser");
  console.log(JSON.stringify(await runHddn({ smoke: process.argv.includes("--smoke"), extended: process.argv.includes("--extended"), browserName: bi > 0 ? process.argv[bi + 1] : "chromium" }), null, 1));
}
