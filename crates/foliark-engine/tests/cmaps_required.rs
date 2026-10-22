//! NEW-88: a native export never runs without the CMaps its document lists (own test binary: nothing is registered here).
mod common;
use common::{fixture, run};
use foliark_engine::Engine;
use foliark_model::{ErrorCode, ExportMode, ExportPolicy, Limits};

#[test]
fn a_native_export_without_the_documents_cmaps_is_refused_not_a_silently_emptied_font() {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e
        .open(fixture("generated/cjk-predefined-cmaps.pdf"), None, false)
        .unwrap();
    let t = e.prepare_export(&doc, ExportMode::Native, ExportPolicy::default());
    let err = run(&mut e, t).unwrap_err();
    assert_eq!(err.code, ErrorCode::AssetLoadFailed);
    assert_eq!(
        err.details.unwrap().feature.as_deref(),
        Some("cmap:Adobe-GB1-UCS2")
    );
}
