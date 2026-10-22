//! Stroked vector removal (spec §5.4, NEW-37): a stroked subpath is judged by its display
//! bounds grown by half the stroke width times the miter limit; inside -> removed, any other
//! crossing -> removed whole and disclosed as collateral (no rectangle cut for strokes).
//! PyMuPDF `get_drawings` ignores miters, caps and non-uniform scale, so strokes are judged on
//! a PyMuPDF RENDER of the region (ink in the input, none in the output) plus fidelity.py for
//! everything outside the regions and the declared collateral.
#[path = "vector_support/mod.rs"]
mod support;
use foliark_engine::Document;
use foliark_model::{ErrorCode, Limits};
use support::{PATH_CELL, TEXT_CELL, page, refusal, region, run, tiled};

fn reg(content: &str, boxes: &[[f64; 4]]) -> (Vec<u8>, Vec<foliark_model::Rect>) {
    let input = page(content, 0);
    let doc = Document::open(input.clone(), None).unwrap();
    let regions = boxes
        .iter()
        .map(|b| region(&doc, b[0], b[1], b[2], b[3]))
        .collect();
    (input, regions)
}

#[test]
fn a_mitered_spike_reaching_into_the_region_removes_the_whole_stroke() {
    // 8 pt wide, 30 degree corner at (200, 305): the miter tip lies 15.5 pt past the vertex,
    // inside the region, while the path's own box ends at x = 200.
    let content = "8 w 10 M 0 0 0 RG 100 331.8 m 200 305 l 100 278.2 l S\n\
                   2 w 20 20 m 60 40 l S";
    let (input, regions) = reg(content, &[[205.0, 295.0, 230.0, 315.0]]);
    let r = run("miter", input, &regions);
    assert_eq!((r.report.subpaths_removed, r.report.paths_cut), (1, 0));
    assert_eq!(r.report.collateral.len(), 1);
    r.assert_region_blank(0);
    r.assert_rest_unchanged();
    assert_eq!(r.output_paths(), (0, 1), "the far stroke stays");
}

#[test]
fn a_dashed_curve_through_a_region_goes_whole() {
    // The curve passes (150, 341) at t = 0.5; its dashes are gone everywhere, gaps included.
    let content = "3 w [6 3] 0 d 0 0 1 RG 40 340 m 100 380 200 300 260 350 c S\n\
                   [] 0 d 1 w 0 0 0 RG 20 20 m 280 30 l S";
    let (input, regions) = reg(content, &[[120.0, 330.0, 160.0, 360.0]]);
    let r = run("dashed", input, &regions);
    assert_eq!(
        (r.report.subpaths_removed, r.report.collateral.len()),
        (1, 1)
    );
    r.assert_region_blank(0);
    r.assert_rest_unchanged();
    assert_eq!(r.output_paths(), (0, 1));
}

#[test]
fn a_non_uniform_scale_widens_the_stroke_along_its_stretched_axis() {
    // x is stretched 10 times: the 1 pt line is 10 pt wide across x = 195..205. The region
    // starts at 202, 2 pt from the geometry's box; with miter limit 1, 0.5 * sqrt(det) = 1.6 pt
    // of reach would miss it.
    let content = "q 10 0 0 1 0 0 cm 1 w 1 M 0 0 0 RG 20 100 m 20 200 l S Q";
    let (input, regions) = reg(content, &[[202.0, 130.0, 215.0, 170.0]]);
    let r = run("stretched", input, &regions);
    assert_eq!(r.report.subpaths_removed, 1);
    r.assert_region_blank(0);
    assert_eq!(r.output_paths(), (0, 0));
}

#[test]
fn a_square_cap_corner_past_the_miter_factor_is_still_inside_the_reach() {
    // Miter limit 1 and a 20 pt square-capped line at 45 degrees: the cap corner is 14.1 pt
    // from the end point, the miter factor alone would give 10 pt.
    let content = "20 w 1 M 2 J 0 0 0 RG 50 50 m 150 150 l S";
    let (input, regions) = reg(content, &[[161.0, 140.0, 175.0, 155.0]]);
    let r = run("cap", input, &regions);
    assert_eq!(r.report.subpaths_removed, 1);
    r.assert_region_blank(0);
}

#[test]
fn a_stroked_rectangle_is_never_cut_but_its_fill_only_sibling_is() {
    // `B` (fill + stroke) crosses region 0: removed whole, collateral. The fill-only green
    // rectangle inside region 1 is cut as in NEW-36.
    let content = "0 0 1 rg 1 0 0 RG 6 w 50 100 100 100 re B\n\
                   0 0.6 0 rg 160 250 100 60 re f";
    let (input, regions) = reg(
        content,
        &[[120.0, 140.0, 170.0, 160.0], [200.0, 270.0, 230.0, 290.0]],
    );
    let r = run("fill-stroke", input, &regions);
    assert_eq!((r.report.subpaths_removed, r.report.paths_cut), (1, 1));
    assert_eq!(r.report.collateral.len(), 1);
    r.assert_region_blank(0);
    r.assert_region_blank(1);
    r.assert_rest_unchanged();
}

#[test]
fn an_inside_stroke_is_removed_without_collateral_and_a_far_one_stays() {
    let content = "2 w 100 100 m 120 120 l S\n250 350 m 280 380 l S";
    let (input, regions) = reg(content, &[[90.0, 90.0, 130.0, 130.0]]);
    let r = run("inside", input, &regions);
    assert_eq!(
        (r.report.subpaths_removed, r.report.collateral.len()),
        (1, 0)
    );
    r.assert_region_blank(0);
    r.assert_rest_unchanged();
    assert_eq!(r.output_paths(), (0, 1));
}

#[test]
fn a_singular_transform_stroke_is_removed() {
    // `1 0 0 0 100 300 cm` collapses the line onto y = 300; its reach is still measured.
    let content = "q 1 0 0 0 100 300 cm 4 w 0 0 m 100 0 l S Q";
    let (input, regions) = reg(content, &[[120.0, 295.0, 160.0, 305.0]]);
    let r = run("singular-stroke", input, &regions);
    assert_eq!(
        (r.report.subpaths_removed, r.report.collateral.len()),
        (1, 1)
    );
    assert_eq!(r.output_paths(), (0, 0));
}

#[test]
fn stroke_paint_from_a_tiling_pattern_with_text_is_refused() {
    let input = tiled(
        "/Pattern CS /P1 SCN 8 w 50 100 m 250 100 l S",
        TEXT_CELL,
        PATH_CELL,
    );
    let doc = Document::open(input.clone(), None).unwrap();
    let regions = vec![region(&doc, 100.0, 90.0, 160.0, 110.0)];
    let e = refusal(input, &regions, Limits::default());
    assert_eq!(e.code, ErrorCode::RedactPatternUnsupported);
}

#[test]
fn a_huge_stroked_path_is_bounded_by_the_work_budget() {
    let content: String = (0..40_000)
        .map(|i| format!("{} 10 m {} 12 l ", i % 250, i % 250))
        .collect::<String>()
        + "S";
    let input = page(&content, 0);
    let doc = Document::open(input.clone(), None).unwrap();
    let regions = vec![region(&doc, 100.0, 5.0, 140.0, 30.0)];
    let limits = Limits {
        max_work_units: 20_000,
        ..Limits::default()
    };
    assert_eq!(
        refusal(input, &regions, limits).code,
        ErrorCode::LimitExceeded
    );
}
