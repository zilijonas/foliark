#!/usr/bin/env node
// CycloneDX 1.5 SBOM for the shipped artifact: the production crates compiled into the wasm (docs/provenance/dependencies.json,
// checksums from Cargo.lock), the embedded data (Foxit fonts, ICC profiles) and no npm runtime dependencies.
// Usage: node tools/release/sbom.mjs <out.json> [tarball]   Deterministic: no timestamp, sorted components.
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../..', import.meta.url));
const [out, tgz] = process.argv.slice(2);
if (!out) { console.error('usage: sbom.mjs <out.json> [tarball]'); process.exit(2); }
const read = (p) => readFileSync(root + p, 'utf8');

const inv = JSON.parse(read('docs/provenance/dependencies.json'));
const pkg = JSON.parse(read('packages/sdk/package.json'));
if (inv.npm.runtime_dependencies !== 0 || Object.keys(pkg.dependencies ?? {}).length) {
  throw new Error('npm runtime dependencies are not empty: extend sbom.mjs before releasing');
}
const sums = new Map();
for (const m of read('Cargo.lock').matchAll(/name = "([^"]+)"\nversion = "([^"]+)"\n(?:source = "[^"]+"\n)?checksum = "([0-9a-f]{64})"/g)) {
  sums.set(`${m[1]}@${m[2]}`, m[3]);
}
const spdx = (l) => (/^[A-Za-z0-9.\-+]+$/.test(l) ? { license: { id: l } } : { expression: l });

const components = inv.packages
  .filter((p) => p.ecosystem === 'cargo' && p.kind === 'prod')
  .map((p) => {
    const sha = sums.get(`${p.name}@${p.version}`);
    const first = p.source === 'crates.io';
    return {
      type: 'library', 'bom-ref': `pkg:cargo/${p.name}@${p.version}`, name: p.name, version: p.version,
      purl: `pkg:cargo/${p.name}@${p.version}`,
      licenses: p.license && p.license !== 'UNLICENSED' ? [spdx(p.license)] : [{ license: { name: 'UNLICENSED (owner decision pending, NEW-73)' } }],
      ...(sha ? { hashes: [{ alg: 'SHA-256', content: sha }] } : {}),
      properties: [{ name: 'foliark:source', value: first ? 'crates.io' : 'workspace or vendored (see patches/hayro/README.md)' }],
    };
  });
components.push(
  { type: 'data', 'bom-ref': 'data:foxit-standard-14-fonts', name: 'Standard 14 fonts (Foxit, from PDFium)', licenses: [{ license: { name: 'BSD-style, PDFium Authors (LICENSE_FOXIT)' } }] },
  { type: 'data', 'bom-ref': 'data:icc-profiles', name: 'ICC profiles CGATS001Compat-v2-micro, LAB, ProPhoto-v2-micro', licenses: [{ license: { id: 'CC0-1.0' } }] },
);
components.sort((a, b) => a['bom-ref'].localeCompare(b['bom-ref']));

const rootRef = `pkg:npm/%40zilijonas/foliark@${pkg.version}`;
const bom = {
  bomFormat: 'CycloneDX', specVersion: '1.5', version: 1,
  metadata: {
    tools: { components: [{ type: 'application', name: 'foliark tools/release/sbom.mjs' }] },
    component: {
      type: 'library', 'bom-ref': rootRef, name: '@zilijonas/foliark', version: pkg.version, purl: rootRef,
      licenses: [{ license: { id: pkg.license } }],
      ...(tgz ? { hashes: [{ alg: 'SHA-256', content: createHash('sha256').update(readFileSync(tgz)).digest('hex') }] } : {}),
    },
  },
  components,
  dependencies: [{ ref: rootRef, dependsOn: components.map((c) => c['bom-ref']) }],
};
writeFileSync(out, JSON.stringify(bom, null, 2) + '\n');
console.log(`${out}: ${components.length} components (npm runtime dependencies: 0)`);
