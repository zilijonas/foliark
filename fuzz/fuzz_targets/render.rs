#![no_main]
use foliark_engine::RenderOptions;
use libfuzzer_sys::fuzz_target;

// maxRenderPixels is 250k, so page 0 at 0.5 either fits or is a typed LIMIT_EXCEEDED.
fuzz_target!(|data: &[u8]| {
    if let Ok(doc) = foliark_fuzz::open(data, true)
        && doc.page_count() > 0
    {
        let _ = doc.render_page(0, RenderOptions::at(0.5));
    }
});
