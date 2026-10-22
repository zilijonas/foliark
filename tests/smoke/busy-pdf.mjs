// A one-page PDF whose content stream is `pairs` times "q Q ": about 4 bytes of decoded content per pair that
// compress ~1000:1, so the file is small and valid but interpreting the page keeps the engine busy in one call.
import { deflateSync } from "node:zlib";

export function busyPdf(pairs) {
  const stream = deflateSync(Buffer.from("q Q ".repeat(pairs)), { level: 1 });
  const parts = [Buffer.from("%PDF-1.7\n")];
  const offsets = [];
  const size = () => parts.reduce((s, p) => s + p.length, 0);
  const obj = (n, head, body) => {
    offsets.push(size());
    parts.push(Buffer.from(`${n} 0 obj\n${head}`), ...(body ? [stream, Buffer.from("\nendstream\nendobj\n")] : [Buffer.from("\nendobj\n")]));
  };
  obj(1, "<< /Type /Catalog /Pages 2 0 R >>");
  obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
  obj(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>");
  obj(4, `<< /Filter /FlateDecode /Length ${stream.length} >>\nstream\n`, true);
  const xref = size();
  const rows = offsets.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("");
  parts.push(Buffer.from(`xref\n0 5\n0000000000 65535 f \n${rows}trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`));
  return Buffer.concat(parts);
}
