import { build } from "esbuild";
import { cp, mkdir } from "node:fs/promises";

const out = process.env.OUT;
await build({
  entryPoints: { main: "src/main.ts", worker: "node_modules/@foliark/sdk/dist/worker.js" },
  bundle: true,
  format: "esm",
  target: "es2022",
  outdir: out,
  publicPath: process.env.BASE,
  loader: { ".wasm": "file" },
  assetNames: "[name]-[hash]",
  logLevel: "warning",
});
await mkdir(out, { recursive: true });
await cp("index.html", `${out}/index.html`);
