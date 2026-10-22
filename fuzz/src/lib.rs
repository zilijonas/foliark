//! Shared by every target: tight [`Limits`] so each input stays cheap and any allocation
//! beyond the budget is a finding, not a slow run.
use foliark_engine::Document;
use foliark_model::{Limits, PdfError};

pub fn tight_limits() -> Limits {
    const MIB: u32 = 1 << 20;
    Limits {
        max_input_bytes: MIB,
        max_pages: 8,
        max_objects: 20_000,
        max_stream_decoded_bytes: 4 * MIB,
        max_decode_ratio: 1000,
        max_image_pixels: 1_000_000,
        max_render_pixels: 250_000,
        max_nesting: 8,
        max_work_units: 2_000_000,
        max_output_bytes: 8 * MIB,
        ..Limits::default()
    }
}

/// Open with the tight limits; repaired files are accepted so mutated inputs reach deeper.
pub fn open(data: &[u8], allow_repaired: bool) -> Result<Document, PdfError> {
    Document::open_with_limits(data.to_vec(), None, allow_repaired, tight_limits())
}

/// Pages worth visiting per input (the page limit is 8, three keep an iteration fast).
pub fn pages(doc: &Document) -> std::ops::Range<usize> {
    0..doc.page_count().min(3)
}
