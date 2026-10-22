#![no_main]
use foliark_model::page_geometry;
use libfuzzer_sys::fuzz_target;

// Boxes, /Rotate and /UserUnit from raw bytes (NaN, inf, huge and tiny values come for free).
// The result is a typed error or a geometry whose numbers are all finite.
fuzz_target!(|data: &[u8]| {
    let mut bytes = data.iter().copied().chain(std::iter::repeat(0));
    let mut f64_ = || f64::from_le_bytes(std::array::from_fn(|_| bytes.next().unwrap()));
    let media = [f64_(), f64_(), f64_(), f64_()];
    let crop_box = [f64_(), f64_(), f64_(), f64_()];
    let user_unit = f64_();
    let rotate = i64::from_le_bytes(std::array::from_fn(|_| bytes.next().unwrap()));
    let crop = (data.first().copied().unwrap_or(0) & 1 == 1).then_some(crop_box);
    let Ok(g) = page_geometry(0, media, crop, rotate, user_unit) else {
        return;
    };
    let nums = [g.width, g.height, g.user_unit]
        .into_iter()
        .chain(g.media_box)
        .chain(g.crop_box)
        .chain(g.user_to_display.0)
        .chain(g.display_to_user.0);
    for v in nums {
        assert!(v.is_finite(), "non-finite geometry value {v}");
    }
    assert!(g.width > 0.0 && g.height > 0.0, "empty page accepted");
});
