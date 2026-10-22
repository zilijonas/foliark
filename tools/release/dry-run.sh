#!/usr/bin/env bash
# Local release dry run (NEW-75). Publishes nothing: `npm publish --dry-run` runs against an unreachable registry URL, so it cannot
# reach the network or use credentials. Needs the candidate built (tests/package-consumers/pack.sh dist-pkg, see docs/release/candidate.md).
# Usage: tools/release/dry-run.sh [tarball]    SKIP_RECORDED_HASH=1 skips the candidate.md hash check (CI rebuilds; cross-machine
# byte identity is not shown, so CI records its own sha in SHA256SUMS instead). No heavy work: no memory-queue needed.
set -euo pipefail
cd "$(dirname "$0")/../.."
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok   $*"; }

ver=$(node -p 'require("./packages/sdk/package.json").version')
tgz=${1:-dist-pkg/zilijonas-foliark-$ver.tgz}
[ -f "$tgz" ] || fail "no tarball at $tgz (build: scripts/build-wasm.sh && pnpm build:sdk && tests/package-consumers/pack.sh dist-pkg)"
dir=$(dirname "$tgz")
sha=$(shasum -a 256 "$tgz" | cut -d' ' -f1)

# 1. package bytes
if [ "${SKIP_RECORDED_HASH:-}" = 1 ]; then ok "tarball sha256 $sha (recorded-hash check skipped)"; else
  want=$(grep -oE '`[0-9a-f]{64}`' docs/release/candidate.md | head -1 | tr -d '`')
  [ "$sha" = "$want" ] || fail "tarball sha256 $sha != recorded $want (docs/release/candidate.md)"
  ok "tarball bytes == recorded candidate sha256 $sha"
fi

# 2. contents and manifest
list=$(tar tzf "$tgz")
for f in package.json README.md LICENSE LICENSING.md COMMERCIAL-LICENSING.md LICENSE_FOXIT NOTICE THIRD_PARTY_NOTICES.md SUPPORT_MATRIX.md dist/index.js wasm/foliark_wasm_bg.wasm cmaps/LICENSE.txt; do
  grep -qx "package/$f" <<<"$list" || fail "tarball lacks $f"
done
! grep -iE 'fixture|\.map$|test|\.pdf$' <<<"$list" || fail "tarball contains fixtures, maps, tests or PDFs"
ok "$(wc -l <<<"$list" | tr -d ' ') entries; notices, licences, support matrix, wasm present; no fixtures/maps/tests/PDFs"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
tar xzf "$tgz" -C "$tmp" package/package.json package/LICENSE package/THIRD_PARTY_NOTICES.md package/SUPPORT_MATRIX.md
node -e '
  const m = JSON.parse(require("fs").readFileSync(process.argv[1] + "/package/package.json"));
  if (m.version !== process.argv[2]) throw new Error("tarball version " + m.version);
  if (Object.keys(m.dependencies || {}).length) throw new Error("runtime dependencies present");
  if (m.name !== "@zilijonas/foliark") throw new Error("package name " + m.name);
  if (m.license !== "AGPL-3.0-only") throw new Error("license " + m.license);
  if (m.private) throw new Error("tarball manifest is private");
  console.log("ok   manifest @zilijonas/foliark " + m.version + ", license " + m.license + ", private " + !!m.private + ", runtime dependencies none");
' "$tmp" "$ver"
cmp -s "$tmp/package/THIRD_PARTY_NOTICES.md" packages/sdk/THIRD_PARTY_NOTICES.md || fail "tarball notices differ from packages/sdk"
cmp -s "$tmp/package/SUPPORT_MATRIX.md" docs/support-matrix.md || fail "tarball support matrix differs from docs/support-matrix.md"
node scripts/gen-notices.mjs --check >/dev/null || fail "notices are stale (node scripts/gen-notices.mjs)"
ok "notices up to date and identical to the tarball copy; tarball support matrix == docs/support-matrix.md"

# 3. SBOM and checksums, written next to the tarball (outside the tarball, so its bytes stay the recorded ones)
node tools/release/sbom.mjs "$dir/zilijonas-foliark-$ver.cdx.json" "$tgz" >/dev/null
node -e '
  const [f, sha, inv] = process.argv.slice(1);
  const b = JSON.parse(require("fs").readFileSync(f));
  const want = JSON.parse(require("fs").readFileSync(inv)).packages.filter((p) => p.ecosystem === "cargo" && p.kind === "prod").length + 2;
  if (b.bomFormat !== "CycloneDX" || !b.specVersion) throw new Error("not CycloneDX");
  if (b.components.length !== want) throw new Error("components " + b.components.length + " != " + want);
  if (b.metadata.component.hashes[0].content !== sha) throw new Error("SBOM tarball hash differs");
  if (new Set(b.components.map((c) => c["bom-ref"])).size !== b.components.length) throw new Error("duplicate bom-ref");
  console.log("ok   SBOM CycloneDX " + b.specVersion + ", " + b.components.length + " components (94 crates + 2 data sets), root hash == tarball");
' "$dir/zilijonas-foliark-$ver.cdx.json" "$sha" docs/provenance/dependencies.json
( cd "$dir" && shasum -a 256 "$(basename "$tgz")" "zilijonas-foliark-$ver.cdx.json" > SHA256SUMS && shasum -a 256 -c SHA256SUMS >/dev/null )
ok "SHA256SUMS written and verified"

# 4. npm dry run: unreachable registry, so no network and no credentials are possible
abs=$(cd "$dir" && pwd)/$(basename "$tgz")   # absolute: a bare relative name is parsed by npm as a git/registry spec
out=$(npm publish "$abs" --dry-run --access public --registry http://127.0.0.1:9 --userconfig /dev/null 2>&1) || { echo "$out" >&2; fail "npm publish --dry-run failed"; }
grep -q 'total files: ' <<<"$out" && grep -q '(dry-run)' <<<"$out" || fail "unexpected npm dry-run output"
ok "npm publish --dry-run (registry http://127.0.0.1:9, no auth): $(grep 'total files' <<<"$out" | sed 's/npm notice //')"

# 5. support, policy and process docs
for f in LICENSE CONTRIBUTING.md SECURITY.md CHANGELOG.md docs/support-matrix.md docs/release/process.md docs/release/candidate.md docs/provenance/dependencies.json .github/workflows/release.yml; do
  [ -s "$f" ] || fail "missing $f"
done
grep -q "^## \[$ver\]" CHANGELOG.md || fail "CHANGELOG.md has no [$ver] entry"
head -1 LICENSE | grep -q 'GNU AFFERO GENERAL PUBLIC LICENSE' || fail "LICENSE is not the AGPL text"
cmp -s "$tmp/package/LICENSE" LICENSE || fail "tarball LICENSE differs from the repository LICENSE"
grep -q "tag \`v$ver\`" packages/sdk/README.md || fail "package README does not name the source tag v$ver"
grep -q "git tag\|tag \`v" LICENSING.md || fail "LICENSING.md lacks source correspondence"
[ -s vendor/hayro-syntax/Cargo.toml ] && [ -s patches/hayro/README.md ] || fail "vendored hayro fork or patch register missing"
ok "AGPL LICENSE (tarball copy identical), source tag v$ver named, hayro fork + patches present, CONTRIBUTING, SECURITY, CHANGELOG [$ver], support matrix, process doc, workflow present"

# 6. publication gate status (informational: the real release workflow refuses these)
node -e '
  const m = require("./packages/sdk/package.json");
  const gates = [];
  if (m.private) gates.push("package.json private:true");
  if (m.license !== "AGPL-3.0-only") gates.push("license " + m.license);
  console.log(gates.length ? "gate publication blocked by: " + gates.join(", ") : "gate package metadata allows publication (publication itself: NEW-91, tag v" + m.version + " after the PR merge)");
'
echo "dry run passed; nothing was published"
