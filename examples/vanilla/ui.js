// DOM wiring for the demo page. `start` creates the engine; it runs on the first chosen file, so a page load
// costs no wasm or worker request. Shared by the vanilla page and the bundler consumers in tests/package-consumers.
import { redactAndExport, show } from "./flow.js";

/** @param {() => Promise<import("@foliark/sdk").Foliark>} start */
export function mount(start) {
  const $ = (/** @type {string} */ id) => /** @type {any} */ (document.getElementById(id));
  const status = (/** @type {string} */ s) => ($("status").textContent = s);
  const fail = (/** @type {any} */ e) => status(`${e.code ?? "ERROR"}: ${e.message}`);
  /** @type {Promise<import("@foliark/sdk").Foliark> | undefined} */
  let engine;
  /** @type {import("@foliark/sdk").DocumentHandle | undefined} */
  let doc;

  $("file").onchange = async () => {
    const file = $("file").files[0];
    if (!file) return;
    try {
      status("Opening...");
      engine ??= start();
      const foliark = await engine;
      await doc?.close();
      const shown = await show(foliark, await file.arrayBuffer());
      doc = shown.doc;
      const c = $("page");
      c.width = shown.page.width;
      c.height = shown.page.height;
      c.getContext("2d").putImageData(new ImageData(shown.page.data, shown.page.width, shown.page.height), 0, 0);
      $("text").textContent = shown.text;
      $("word").value = shown.text.split(/\s+/).find(Boolean) ?? "";
      $("redact").disabled = false;
      $("download").hidden = true;
      status(`Page 1 of ${(await doc.info()).pageCount}.`);
    } catch (e) {
      engine = undefined; // a failed start (missing asset) can be retried
      fail(e);
    }
  };

  $("redact").onclick = async () => {
    try {
      const bytes = await redactAndExport(/** @type {any} */ (doc), $("word").value);
      const a = $("download");
      URL.revokeObjectURL(a.href);
      a.href = URL.createObjectURL(new Blob([bytes], { type: "application/pdf" }));
      a.hidden = false;
      status(`Exported ${bytes.byteLength} bytes.`);
    } catch (e) {
      fail(e);
    }
  };
}
