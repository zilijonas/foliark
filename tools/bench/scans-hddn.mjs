// HDDN previews of the synthetic scans (NEW-81): node tools/bench/scans-hddn.mjs [--browser chromium|webkit] [--only ID] [--out FILE]
import { writeFile } from "node:fs/promises";
import { SCAN_FIXTURES } from "./lib.mjs";
import { runHddn } from "./hddn-worker.mjs";
const arg = (k) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : undefined; };
const only = arg("--only") ? SCAN_FIXTURES.filter((f) => f[0] === arg("--only")) : SCAN_FIXTURES;
const res = JSON.stringify(await runHddn({ browserName: arg("--browser") ?? "chromium", only, scales: [1.35, 3] }), null, 1);
if (arg("--out")) await writeFile(arg("--out"), res); else console.log(res);
