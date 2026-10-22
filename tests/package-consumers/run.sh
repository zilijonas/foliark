#!/usr/bin/env bash
# The installed-tarball consumer matrix (NEW-60): pack @zilijonas/foliark, install the TARBALL (no workspace link, no source
# alias) into isolated projects, build each under base / and /app/, type-check, then run every build in chromium.
# Needs wasm + sdk built first and network for npm (vite, esbuild, typescript). Wrap in memory-queue (--need-gb 4);
# do not nest it. Exit 0 = the whole matrix passed.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$here/../..
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
sites=()
npm_i() { npm install --no-audit --no-fund --no-package-lock --loglevel=error "$@"; }

tgz=$("$here/pack.sh" "$work")
echo "tarball $(basename "$tgz") $(wc -c < "$tgz") B sha256 $(shasum -a 256 "$tgz" | cut -d' ' -f1)"
tar tzf "$tgz" | grep -vE '/generated/' | sort | sed 's/^/  /'

for name in vanilla vite esbuild; do
  proj=$work/$name
  cp -R "$here/$name" "$proj"
  if [ "$name" != vanilla ]; then
    cp "$root/examples/vanilla/flow.js" "$root/examples/vanilla/ui.js" "$proj/src/"
    if [ "$name" = vite ]; then sed 's#src="./main.js"#src="./src/main.ts"#' "$root/examples/vanilla/index.html" > "$proj/index.html"
    else cp "$root/examples/vanilla/index.html" "$proj/index.html"; fi
  fi
  (cd "$proj" && npm_i "$tgz" $(node -p 'Object.entries(require("./package.json").devDependencies ?? {}).map(([k,v])=>k+"@"+v).join(" ")'))
  # the installed package must not be a symlink into the monorepo
  [ ! -L "$proj/node_modules/@zilijonas/foliark" ] || { echo "$name: @zilijonas/foliark is a link" >&2; exit 1; }
  sdk=$proj/node_modules/@zilijonas/foliark
  case $name in
    vanilla)
      # no build: the page is examples/vanilla plus the installed dist/ and wasm/ hosted as sdk/
      "$here/stage-vanilla.sh" "$sdk" "$proj/site"
      sites+=("$name:/:$proj/site" "$name:/app/:$proj/site") ;;
    *)
      (cd "$proj" && node_modules/.bin/tsc --noEmit -p .)
      for base in / /app/; do
        out=$proj/out$([ "$base" = / ] && echo -root || echo -app)
        (cd "$proj" && BASE=$base OUT=$out node build.mjs)
        sites+=("$name:$base:$out")
      done ;;
  esac
done

node "$here/check.mjs" "${sites[@]}"
echo "package consumers: ok (${#sites[@]} builds)"
