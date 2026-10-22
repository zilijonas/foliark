// @ts-check
// The demo flow, shared by this page and the bundler consumers in tests/package-consumers (type-checked there
// against the shipped declarations).

/**
 * Open a PDF and return what the page shows: page-1 text and page-1 pixels.
 * @param {import("@foliark/sdk").Foliark} foliark
 * @param {ArrayBuffer} bytes
 */
export async function show(foliark, bytes) {
  const doc = await foliark.open(bytes);
  const { text } = await doc.extractText(0);
  const page = await doc.renderPage(0, { scale: 1 });
  return { doc, text, page };
}

/**
 * Redact every occurrence of `word` (black boxes, text removed) and export a sanitized native PDF.
 * @param {import("@foliark/sdk").DocumentHandle} doc
 * @param {string} word
 * @returns {Promise<Uint8Array>}
 */
export async function redactAndExport(doc, word) {
  const hits = await doc.search(word);
  if (!hits.length) throw new Error(`"${word}" not found`);
  const redactions = hits.map((h, i) => /** @type {const} */ ({ kind: "text", id: `r${i}`, pageIndex: h.pageIndex, start: h.start, end: h.end }));
  const r = await doc.applyChanges({ baseRevision: doc.revision, redactions });
  if (!r.accepted) throw new Error(`redaction refused: ${r.rejected.map((x) => x.code).join(", ")}`);
  return doc.export(await doc.prepareExport({ mode: "native" }));
}
