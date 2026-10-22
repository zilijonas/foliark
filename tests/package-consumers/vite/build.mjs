import { build } from "vite";

await build({
  base: process.env.BASE,
  logLevel: "warn",
  build: { outDir: process.env.OUT, emptyOutDir: true },
  worker: { format: "es" },
});
