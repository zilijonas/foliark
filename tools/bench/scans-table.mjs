// Merge the scan runs into docs/performance/comparison.json (key `scans`) and print the markdown table (NEW-81).
// node tools/bench/scans-table.mjs --hddn H.json --before B.json --after A.json [--fidelity F.json] [--write]
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { ROOT, machine } from "./lib.mjs";

const arg = (k) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : undefined; };
const load = async (k) => (arg(k) ? JSON.parse(await readFile(arg(k), "utf8")) : undefined);
const [hddn, before, after, fidelity] = await Promise.all(["--hddn", "--before", "--after", "--fidelity"].map(load));
const find = (run, op, input) => run.measurements.find((m) => m.op === op && m.input === input && (m.run ?? "warm") === "warm");
const cell = (m) => (m ? String(m.ms) : "n/a");
const ids = after.inputs.map((i) => i.id);
const lines = [];
for (const s of [1.35, 3]) {
  lines.push(`\n#### Scale ${s}${s === 3 ? " (HDDN OCR scale)" : ""}, ms per page, median, ${after.browser}\n`, "| fixture | HDDN (PyMuPDF + PNG) | SDK before | SDK after, 1st render | SDK after, 2nd | SDK after, 3rd and later | 1st vs HDDN |", "|---|---|---|---|---|---|---|");
  for (const id of ids) {
    const h = find(hddn, `GET_PAGE_PREVIEW@${s}`, id), b = find(before, `renderPage@${s} first after open`, id);
    const a = find(after, `renderPage@${s} first after open`, id), r = find(after, `renderPage@${s} repeat`, id), s2 = find(after, `renderPage@${s} second`, id);
    lines.push(`| ${id.replace("scan-", "")} | ${cell(h)} | ${cell(b)} | ${cell(a)} | ${cell(s2)} | ${cell(r)} | ${h && a ? (h.ms / a.ms).toFixed(1) + "x faster" : "n/a"} |`);
  }
}
console.log(lines.join("\n"));
if (process.argv.includes("--write")) {
  const path = join(ROOT, "docs/performance/comparison.json");
  const doc = JSON.parse(await readFile(path, "utf8"));
  const keep = (run) => ({ engine: run.engine, browser: run.browser, wasmSha256: run.wasmSha256, wasmBytes: run.wasmBytes, inputs: run.inputs, measurements: run.measurements.filter((m) => /^(GET_PAGE_PREVIEW|LOAD_PDF|renderPage)/.test(m.op) && (m.run ?? "warm") === "warm") });
  doc.scans = {
    kind: "scanned-page render, NEW-81", createdAt: new Date().toISOString(), machine: machine(),
    fixtures: "tests/fixtures/generators/scans.py (3 pages each, 2480x3508 px, 300 dpi A4, synthetic); sha256 in sdkAfter.inputs",
    hddn: { ...keep(hddn), unavailable: hddn.unavailable.filter((u) => u.op === "GET_PAGE_PREVIEW" || u.op === "LOAD_PDF") },
    sdkBefore: keep(before), sdkAfter: keep(after), fidelity,
    notes: ["HDDN GET_PAGE_PREVIEW = PyMuPDF get_pixmap + PNG encode in Pyodide; SDK = raw RGBA", "HDDN COMMIT_IMPORT is called with allowIncompleteOcr (scanned pages are in the OCR lane); OCR itself is not measured", "SDK 1st = renderPage on a freshly opened document (image left to the renderer, only remembered); 2nd = same page again (image reduced and kept, hayro P17); 3rd and later = decode kept"],
  };
  await writeFile(path, JSON.stringify(doc, null, 1) + "\n");
  console.error(`wrote scans into ${path}`);
}
