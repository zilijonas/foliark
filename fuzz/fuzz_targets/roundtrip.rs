#![no_main]
use foliark_engine::writer::Writer;
use foliark_model::ErrorCode;
use libfuzzer_sys::fuzz_target;

// A file the writer accepts must reopen strictly with the same page count.
fuzz_target!(|data: &[u8]| {
    let Ok(doc) = foliark_fuzz::open(data, true) else {
        return;
    };
    let Ok(out) = Writer::new(doc.reader(), foliark_fuzz::tight_limits()).write() else {
        return;
    };
    match foliark_fuzz::open(&out, false) {
        Ok(again) => assert_eq!(again.page_count(), doc.page_count(), "page count changed"),
        Err(e) if e.code == ErrorCode::LimitExceeded => {}
        Err(e) => panic!("writer output does not reopen: {e:?}"),
    }
});
