#![no_main]
use libfuzzer_sys::fuzz_target;

// Strict and repair-tolerant open plus the preflight scan; every failure must be typed.
fuzz_target!(|data: &[u8]| {
    if let Ok(doc) = foliark_fuzz::open(data, false) {
        let _ = doc.reader().info();
    }
    if let Ok(doc) = foliark_fuzz::open(data, true) {
        let _ = doc.reader().info();
    }
});
