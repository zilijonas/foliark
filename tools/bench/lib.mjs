// Shared bench helpers: static server with per-path byte accounting, stats, fixtures, machine info.
import { createHash } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import { createReadStream, existsSync } from "node:fs";
import { mkdir, readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { cpus, loadavg, totalmem } from "node:os";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

export const ROOT = fileURLToPath(new URL("../../", import.meta.url));
export const BENCH = join(ROOT, "tools/bench");
// HDDN@2fe0f9a built worktree: $HDDN_DIST, else <root>/.worktrees/hddn-2fe0f9a or the sibling worktree of a linked checkout.
export const HDDN_DIST = process.env.HDDN_DIST ?? [join(ROOT, ".worktrees/hddn-2fe0f9a/dist"), join(ROOT, "../hddn-2fe0f9a/dist")].find(existsSync) ?? join(ROOT, ".worktrees/hddn-2fe0f9a/dist");
const MIME = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".wasm": "application/wasm", ".json": "application/json", ".zip": "application/zip", ".whl": "application/octet-stream", ".ttf": "font/ttf", ".css": "text/css", ".pdf": "application/pdf" };

export const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");
export const median = (a) => { const s = [...a].sort((x, y) => x - y); const m = s.length >> 1; return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2; };
export const p95 = (a) => { const s = [...a].sort((x, y) => x - y); return s[Math.min(s.length - 1, Math.ceil(0.95 * s.length) - 1)]; };
const r1 = (x) => Math.round(x * 10) / 10;
export const stat1 = (a) => ({ ms: r1(median(a)), p95: r1(p95(a)), n: a.length });

// mounts: [{prefix, dir}] longest prefix wins. Serves identity-encoded, cacheable (max-age) so warm runs hit the HTTP cache.
// Same-origin + COOP/COEP so performance.measureUserAgentSpecificMemory is available. bytes: Map(path -> bytes sent).
export async function serve(mounts) {
  const bytes = new Map();
  const sorted = [...mounts].sort((a, b) => b.prefix.length - a.prefix.length);
  const server = createServer(async (req, res) => {
    const url = decodeURIComponent(new URL(req.url, "http://x").pathname);
    const m = sorted.find((x) => url.startsWith(x.prefix));
    try {
      if (!m || url.includes("..")) throw new Error("no mount");
      let file = join(m.dir, normalize(url.slice(m.prefix.length)));
      if ((await stat(file)).isDirectory()) file = join(file, "index.html");
      const size = (await stat(file)).size;
      res.writeHead(200, { "content-type": MIME[extname(file)] ?? "application/octet-stream", "content-length": size, "cache-control": "public, max-age=3600", "cross-origin-opener-policy": "same-origin", "cross-origin-embedder-policy": "require-corp", "cross-origin-resource-policy": "same-origin" });
      res.on("finish", () => bytes.set(url, (bytes.get(url) ?? 0) + size));
      createReadStream(file).pipe(res);
    } catch { res.writeHead(404).end(); }
  }).listen(0, "127.0.0.1");
  await new Promise((r) => server.once("listening", r));
  return { origin: `http://127.0.0.1:${server.address().port}`, bytes, close: () => server.close() };
}

export function sumBytes(map) {
  const out = { runtime: 0, fonts: 0, ocr: 0, app: 0 };
  for (const [p, n] of map) out[p.startsWith("/pyodide/") ? "runtime" : p.startsWith("/fonts/") ? "fonts" : p.startsWith("/tesseract/") ? "ocr" : "app"] += n;
  return out;
}

const GENERATED = { generated: { script: "make-native30.py", pages: 30 }, "generated-image": { script: "make-imageheavy.py", pages: 6 } };
export async function loadFixtures(want) {
  const manifest = JSON.parse(await readFile(join(ROOT, "tests/fixtures/manifest.json"), "utf8"));
  const byId = Object.fromEntries(manifest.files.map((f) => [f.id, f]));
  await mkdir(join(BENCH, ".cache"), { recursive: true });
  const out = [];
  for (const [id, source, role] of want) {
    let path = source;
    const scan = source.startsWith("scan:") ? source.slice(5) : null;
    if (scan) {
      path = join(SCANS, `scan-${scan}.pdf`);
      if (!existsSync(path)) execFileSync(join(ROOT, "tests/oracles/.venv/bin/python"), [join(ROOT, "tests/fixtures/generators/scans.py"), SCANS]);
    } else if (GENERATED[source]) { path = join(BENCH, `.cache/${id}.pdf`); execFileSync("python3", [join(BENCH, GENERATED[source].script), path]); }
    else path = join(ROOT, "tests/fixtures", byId[source].path);
    const buf = await readFile(path);
    out.push({ id, role, path, buf, sha256: sha256(buf), bytes: buf.length, pages: scan ? SCAN_PAGES : GENERATED[source]?.pages ?? byId[source].pages, origin: scan ? "tests/fixtures/generators/scans.py (Pillow, synthetic)" : GENERATED[source] ? `tools/bench/${GENERATED[source].script} (PyMuPDF, dev-only)` : `tests/fixtures/${byId[source].path}` });
  }
  return out;
}
const SCANS = join(BENCH, ".cache/scans");
const SCAN_PAGES = 3;
// NEW-81: synthetic 300 dpi A4 scans, one image codec each (tests/fixtures/generators/scans.py)
export const SCAN_FIXTURES = ["jpeg-baseline-rgb", "jpeg-baseline-gray", "jpeg-progressive-rgb", "jpeg-progressive-gray", "jbig2", "ccitt-g4", "flate-gray"].map((k) => [`scan-${k}`, `scan:${k}`, "scanned page"]);
export const FIXTURES = [
  ["native-small", "metadata-xmp", "small native text"],
  ["native-30p", "generated", "30-page native text"],
  ["image-heavy", "shared-image", "image-heavy"],
  ["rotation-crop", "rotation-crop", "rotation + crop box"],
];
// comparison.json adds a real image-heavy document (the baseline "image-heavy" is a 61 KB 2-page file)
export const EXTENDED_FIXTURES = [...FIXTURES, ["image-heavy-6p", "generated-image", "image-heavy 6p 300dpi JPEG"]];

export function machine() {
  const sh = (c, a) => spawnSync(c, a, { encoding: "utf8" }).stdout.trim();
  return { cpu: cpus()[0].model, cores: cpus().length, ramGB: Math.round(totalmem() / 2 ** 30), macOS: sh("sw_vers", ["-productVersion"]), arch: process.arch, loadAvg1mAtStart: Math.round(loadavg()[0] * 10) / 10 };
}
export const gitSha = () => spawnSync("git", ["-C", ROOT, "rev-parse", "--short", "HEAD"], { encoding: "utf8" }).stdout.trim();

// Resident set size (bytes) of every descendant of this node process (the browser tree). RSS, not JS heap.
export function treeRssBytes() {
  const rows = spawnSync("ps", ["-axo", "pid=,ppid=,rss="], { encoding: "utf8" }).stdout.trim().split("\n").map((l) => l.trim().split(/\s+/).map(Number));
  const kids = new Map(); for (const [pid, ppid, rss] of rows) { if (!kids.has(ppid)) kids.set(ppid, []); kids.get(ppid).push([pid, rss]); }
  let total = 0; const walk = (pid) => { for (const [c, rss] of kids.get(pid) ?? []) { total += rss * 1024; walk(c); } };
  walk(process.pid); return total;
}

export const OUT_DIR = join(BENCH, ".cache/out"); // exported PDFs, checked by verify-outputs.py
export const EDIT_LINES = 10; // text lines of page 0 used as redaction regions / paragraph edit target
