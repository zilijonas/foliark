import { defineConfig } from "playwright/test";

// One worker: each browser instance holds a 5 MB wasm plus a document; never run engines in parallel (24 GB host).
export default defineConfig({
  testDir: ".",
  workers: 1,
  timeout: 90_000,
  reporter: "list",
  outputDir: "../../test-results",
  projects: ["chromium", "firefox", "webkit"].map((name) => ({ name, use: { browserName: name as "chromium" } })),
});
