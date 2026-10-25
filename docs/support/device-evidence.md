# Device evidence (NEW-64)

Real-browser and real-device runs of Foliark through HDDN (https://hddn.app). Playwright WebKit is not Safari evidence and is recorded only in `docs/support-matrix.md`.

## Phones and tablets (owner, 2026-10-25)

- **Build:** hddn.app production `3bd8dc6` on `@zilijonas/foliark` 0.1.4. This is the build that was live when the owner tested; 0.1.5 followed the same day.
- **Who ran it and where:** the owner, on the owner's own phone and tablet, against live hddn.app.
- **Checklist:** the 9 steps in the NEW-64 board note:
  1. first load
  2. text PDF: select, detect, zoom and scroll
  3. mark and export
  4. edit a paragraph and export
  5. scan with OCR and export
  6. form/comments PDF with the export warning
  7. 20+ page file
  8. prepared offline in airplane mode
  9. rotate and switch apps mid-edit
- **Result as reported:** "everything works flawlessly". No step failed, and there was no crash or blank screen.
- **Not recorded:** device models, OS and browser versions, timings, and which tablet was used. The iPhone, iPad and Android rows in the support matrix therefore read "functional, through HDDN", and no performance or memory claim is made for phones.

## Desktop Safari (macOS, real browser)

Checks done during the HDDN integration, on the owner's documents. The documents are read in place and none of their text is kept. Board notes hold the details.

- **NEW-99 and the real-document fixes after it:** local HDDN bound to a local Foliark build: open, find, edit, redact and export on the owner's iOS-scan and Word PDFs.
- **NEW-113, after the 0.1.4 deploy (2026-10-25):**
  - inline bold and italic are kept in the preview and in the export
  - the hyphen survives
  - 14 dates and the name are redacted
  - AirPods file: 12 findings, export clean
- **NEW-114:** the new similar-font prompt wording is live, and the editor overlay follows the engine's styles.
- **NEW-116, local 0.1.5 build:** in each of these cases the typed text gets one style:
  - select-all and retype
  - retype from a bold word to the end
  - a one-letter edit inside a bold word stays bold

## Not covered

- No recorded device models or versions.
- No timings on phones.
- No memory or thermal measurements.
- No Windows or Linux browsers.

A new device becomes a support-matrix row only with its own receipt.
