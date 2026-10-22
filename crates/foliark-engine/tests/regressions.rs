//! Every minimized fuzz finding (tests/regressions, manifest.json) must give a typed result and
//! never panic, through the same calls and tight limits as the fuzz targets (fuzz/).
use foliark_engine::writer::{Writer, reemit};
use foliark_engine::{Document, RenderOptions};
use foliark_model::{ErrorCode, ExtractOptions, Limits, ObjectId, page_geometry};
use std::path::Path;

fn tight() -> Limits {
    const MIB: u32 = 1 << 20;
    Limits {
        max_input_bytes: MIB,
        max_pages: 8,
        max_objects: 20_000,
        max_stream_decoded_bytes: 4 * MIB,
        max_image_pixels: 1_000_000,
        max_render_pixels: 250_000,
        max_nesting: 8,
        max_work_units: 2_000_000,
        max_output_bytes: 8 * MIB,
        ..Limits::default()
    }
}

fn open(data: &[u8], repaired: bool) -> Result<Document, foliark_model::PdfError> {
    Document::open_with_limits(data.to_vec(), None, repaired, tight())
}

fn pdf(data: &[u8]) {
    if let Ok(d) = open(data, false) {
        let _ = d.reader().info();
    }
    let Ok(doc) = open(data, true) else { return };
    let _ = doc.reader().info();
    let opts = ExtractOptions {
        chars: true,
        include_ocg_hidden: true,
    };
    for i in 0..doc.page_count().min(3) {
        let _ = doc.page_geometry(i);
        let _ = doc.page_model(i);
        let _ = doc.extract_text(i, &opts);
    }
    if doc.page_count() > 0 {
        let _ = doc.render_page(0, RenderOptions::at(0.5));
    }
    if let Ok(out) = Writer::new(doc.reader(), tight()).write() {
        match open(&out, false) {
            Ok(again) => assert_eq!(again.page_count(), doc.page_count()),
            Err(e) => assert_eq!(
                e.code,
                ErrorCode::LimitExceeded,
                "writer output must reopen"
            ),
        }
    }
}

/// `sel` byte, then a decoded content stream: re-emission is typed and idempotent.
fn content(input: &[u8]) -> Option<ErrorCode> {
    let (&sel, data) = input.split_first().unwrap();
    let obj = ObjectId {
        num: 1,
        generation: 0,
    };
    let start = (sel as usize * data.len()) / 256;
    let end = (start + 1 + (sel as usize % 7)).min(data.len());
    let _ = reemit(data, obj, &[(start..end, b"q Q".to_vec())]);
    match reemit(data, obj, &[]) {
        Ok(once) => {
            assert_eq!(once, reemit(&once, obj, &[]).unwrap(), "not idempotent");
            None
        }
        Err(e) => Some(e.code),
    }
}

/// Little-endian f64 x 8 (media, crop), user unit, rotate; first byte bit 0 selects the crop.
fn geometry(data: &[u8]) {
    let mut b = data.iter().copied().chain(std::iter::repeat(0));
    let mut f = || f64::from_le_bytes(std::array::from_fn(|_| b.next().unwrap()));
    let media = [f(), f(), f(), f()];
    let crop = [f(), f(), f(), f()];
    let unit = f();
    let rotate = i64::from_le_bytes(std::array::from_fn(|_| b.next().unwrap()));
    let crop = (data[0] & 1 == 1).then_some(crop);
    if let Ok(g) = page_geometry(0, media, crop, rotate, unit) {
        assert!(g.width > 0.0 && g.height > 0.0, "empty page accepted");
    }
}

#[test]
fn regressions_have_typed_results() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/regressions");
    let mut n = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let data = std::fs::read(&path).unwrap();
        match path.extension().and_then(|e| e.to_str()) {
            Some("pdf") => pdf(&data),
            Some("bin") if name.starts_with("content_reemit-") => {
                // Ambiguous damaged inline images are refused, never written.
                let refused = content(&data);
                assert!(refused.is_none() || refused == Some(ErrorCode::MalformedPdf));
                if name == "content_reemit-3.bin" {
                    assert_eq!(refused, Some(ErrorCode::MalformedPdf));
                }
            }
            Some("bin") if name.starts_with("geometry-") => geometry(&data),
            Some("json") => continue,
            _ => panic!("unrecognized regression file {name}"),
        }
        n += 1;
    }
    assert!(n >= 4, "regression corpus went missing ({n})");
}
