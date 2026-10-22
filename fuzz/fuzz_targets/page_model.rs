#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(doc) = foliark_fuzz::open(data, true) {
        for i in foliark_fuzz::pages(&doc) {
            let _ = doc.page_geometry(i);
            let _ = doc.page_model(i);
        }
    }
});
