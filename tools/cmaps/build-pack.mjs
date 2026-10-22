#!/usr/bin/env node
// Rebuild packages/sdk/cmaps from the two Adobe repositories (BSD-3-Clause), verbatim and byte for byte.
// Usage: node tools/cmaps/build-pack.mjs <cmap-resources checkout> <mapping-resources-pdf checkout>
// Pinned upstream commits (checkouts must match): adobe-type-tools/cmap-resources f5cf3bca7fdfeaceb77aa82847e974f2306c20b4,
// adobe-type-tools/mapping-resources-pdf 2dd5e53fb74a01718b9dfd448a0d1cce6fff2aa5. The file set is exactly the predefined
// names hayro-cmap knows (CMapName), minus Identity-H/V, which need no file.
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync, rmSync } from "node:fs";
import { join } from "node:path";

const [cmapRes, mapRes] = process.argv.slice(2);
if (!cmapRes || !mapRes) { console.error("usage: build-pack.mjs <cmap-resources> <mapping-resources-pdf>"); process.exit(2); }
const out = new URL("../../packages/sdk/cmaps/", import.meta.url).pathname;
const names = `83pv-RKSJ-H 90ms-RKSJ-H 90ms-RKSJ-V 90msp-RKSJ-H 90msp-RKSJ-V 90pv-RKSJ-H Add-RKSJ-H Add-RKSJ-V B5pc-H B5pc-V
CNS-EUC-H CNS-EUC-V ETen-B5-H ETen-B5-V ETenms-B5-H ETenms-B5-V EUC-H EUC-V Ext-RKSJ-H Ext-RKSJ-V GB-EUC-H GB-EUC-V GBK-EUC-H
GBK-EUC-V GBK2K-H GBK2K-V GBKp-EUC-H GBKp-EUC-V GBpc-EUC-H GBpc-EUC-V H HKscs-B5-H HKscs-B5-V KSC-EUC-H KSC-EUC-V KSCms-UHC-H
KSCms-UHC-HW-H KSCms-UHC-HW-V KSCms-UHC-V KSCpc-EUC-H UniCNS-UCS2-H UniCNS-UCS2-V UniCNS-UTF16-H UniCNS-UTF16-V UniGB-UCS2-H
UniGB-UCS2-V UniGB-UTF16-H UniGB-UTF16-V UniJIS-UCS2-H UniJIS-UCS2-HW-H UniJIS-UCS2-HW-V UniJIS-UCS2-V UniJIS-UTF16-H
UniJIS-UTF16-V UniKS-UCS2-H UniKS-UCS2-V UniKS-UTF16-H UniKS-UTF16-V V`.split(/\s+/);
const ucs2 = ["Adobe-CNS1-UCS2", "Adobe-GB1-UCS2", "Adobe-Japan1-UCS2", "Adobe-Korea1-UCS2"];

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
const find = (root, name, sub) => {
  const hits = readdirSync(root).filter((d) => d !== "deprecated").map((d) => join(root, d, sub, name)).filter((f) => { try { return statSync(f).isFile(); } catch { return false; } });
  if (hits.length !== 1) throw new Error(`${name}: ${hits.length} candidates`);
  return hits[0];
};
const manifest = {};
const put = (name, src) => {
  copyFileSync(src, join(out, `${name}.cmap`));
  const b = readFileSync(src);
  manifest[name] = { bytes: b.length, sha256: createHash("sha256").update(b).digest("hex") };
};
for (const n of names) put(n, find(cmapRes, n, "CMap"));
for (const n of ucs2) put(n, join(mapRes, "pdf2unicode", n));
copyFileSync(join(cmapRes, "LICENSE.md"), join(out, "LICENSE.txt"));
writeFileSync(join(out, "MANIFEST.json"), JSON.stringify({ license: "BSD-3-Clause", copyright: "Adobe", files: manifest }, null, 1) + "\n");
console.log(`${Object.keys(manifest).length} cmaps, ${Object.values(manifest).reduce((s, f) => s + f.bytes, 0)} bytes`);
