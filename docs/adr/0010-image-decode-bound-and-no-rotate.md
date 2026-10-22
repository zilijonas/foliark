# ADR 0010: image decode bound and NoRotate annotations

Status: accepted (NEW-94). Amends spec sections 8.2 and 9; no other contract changes.

## Image streams are bounded by what they declare

`maxStreamDecodedBytes` (128 MiB) is a per-stream cap. A legitimate 9000 x 9000 RGB scan decodes to 243 MB, under `maxImagePixels` (10^8) but over that cap, so HDDN's public `mixed-multipage` file was refused with `LIMIT_EXCEEDED maxStreamDecodedBytes`.

Decision: for an image XObject the cap is `max(maxStreamDecodedBytes, declared size)`, where declared size is `(ceil(width x components x bpc / 8) + 1) x height` (components: 1 for gray and masks, 3 for DeviceRGB/CalRGB, otherwise 4; bpc at most 16; the extra byte per row is the PNG predictor). `maxImagePixels` is checked first, so the bound is at most 10^8 x 8 bytes in the worst case and about 300 to 400 MB for 8-bit colour. Everything else keeps the generic cap and the ratio check. The decoder still stops one byte past the cap, so a stream that inflates beyond its declaration, or a declared-small image, is refused as before. The redaction image budget (`redaction/image.rs`) charged width x height x 4 against the stream cap as well; it now relies on `maxImagePixels` and the work budget.

Rejected: raising `maxStreamDecodedBytes` (every content stream and font would inherit the larger allowance) and removing the check for images (a lying stream would be unbounded).

## NoRotate annotations

Annotation flag 16 (NoRotate): the appearance is not rotated with the page and the upper-left corner of `/Rect` (in the unrotated page) stays where it is (PDF 32000 12.5.3). Flattening placed every appearance by the plain BBox to Rect mapping, so a NoRotate note on a page with `/Rotate` 90, 180 or 270 turned with the page and moved; HDDN's pixel check measured 0.67 to 0.78 percent. Flattening now composes the page rotation into the placement matrix for those annotations. MuPDF and Poppler render the input the same way; the `sanitize_interactive` regression compares against MuPDF.

Off-page removal (`sanitize/visibility.rs`) was investigated for the same file and is correct: with the annotations stripped, input and output render identically, and a fixture with elements straddling each CropBox edge on every rotation keeps all of them.

The budget scan counts the inflated size of a Flate-only image in a 64 KiB buffer instead of decoding it (a 243 MB scan was held, with its predictor output, just to be measured). The raster renderer's annotation placement (`vendor/hayro-interpret`, patch P20) got the same NoRotate rule and a fix for the translation of matrix A, which was not scaled with the box.
