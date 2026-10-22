#![no_main]
use foliark_model::ObjectId;
use libfuzzer_sys::fuzz_target;

// Raw bytes as a decoded content stream. Re-emission must not panic, and emitting its own
// output again must change nothing. The first byte picks one substitution range.
fuzz_target!(|input: &[u8]| {
    let Some((&sel, data)) = input.split_first() else {
        return;
    };
    let obj = ObjectId {
        num: 1,
        generation: 0,
    };
    let start = (sel as usize * data.len()) / 256;
    let end = (start + 1 + (sel as usize % 7)).min(data.len());
    let _ = foliark_engine::writer::reemit(data, obj, &[(start..end, b"q Q".to_vec())]);
    if let Ok(once) = foliark_engine::writer::reemit(data, obj, &[]) {
        let twice = foliark_engine::writer::reemit(&once, obj, &[]).expect("own output re-emits");
        assert_eq!(once, twice, "re-emission is not idempotent");
    }
});
