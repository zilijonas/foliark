//! Filled vector removal (spec §5.4, NEW-36): (a) inside -> removed, (b) a lone axis-aligned
//! rectangle -> its exact difference, (c) any other crossing subpath -> removed whole and
//! reported as collateral. Judged on the OUTPUT by PyMuPDF `get_drawings` (objects.py), a
//! PyMuPDF render (fidelity.py outside the regions and the collateral) and the semantic model.
#[path = "vector_support/mod.rs"]
mod support;
use foliark_engine::Document;
use foliark_model::{ErrorCode, Limits};
use support::{PATH_CELL, TEXT_CELL, circle, page, refusal, region, run, tiled};

fn reg(content: &str, rotate: i32, boxes: &[[f64; 4]]) -> (Vec<u8>, Vec<foliark_model::Rect>) {
    let input = page(content, rotate);
    let doc = Document::open(input.clone(), None).unwrap();
    let regions = boxes
        .iter()
        .map(|b| region(&doc, b[0], b[1], b[2], b[3]))
        .collect();
    (input, regions)
}

#[test]
fn partial_overlap_cuts_the_rectangle_and_leaves_its_neighbours_alone() {
    // Region 1 sits inside the blue rectangle (four pieces); region 2 crosses the right edge of
    // the first red rectangle (three pieces); the second red rectangle shares its path and the
    // green one is far away: both stay byte for byte.
    let content = "0 0 1 rg 50 100 200 100 re f\n\
                   1 0 0 rg 20 320 60 40 re 200 320 60 40 re f\n\
                   0 0.6 0 rg 20 20 260 30 re f";
    let (input, regions) = reg(
        content,
        0,
        &[[100.0, 140.0, 160.0, 180.0], [60.0, 330.0, 100.0, 350.0]],
    );
    let r = run("partial", input, &regions);
    assert_eq!((r.report.rects_cut, r.report.subpaths_removed), (2, 0));
    assert!(r.report.collateral.is_empty());
    r.assert_no_geometry_in_regions();
    r.assert_rest_unchanged();
    r.assert_region_blank(0);
    r.assert_region_blank(1);
}

#[test]
fn cuts_are_exact_under_rotation_flip_nonuniform_scale_and_page_rotate() {
    // Rotated 90 degrees, then y-flipped and scaled 2 x 0.5. The cut is written in the
    // stream's own space; any error shows as a pixel difference outside the regions.
    let content = "q 0 1 -1 0 300 0 cm 1 0 0 rg 20 20 150 100 re f Q\n\
                   q 2 0 0 -0.5 10 390 cm 0 0.5 0 rg 20 100 100 80 re f Q";
    // Rect 1 covers x 180..280, y 20..170 (user); rect 2 covers x 50..250, y 300..340.
    for rotate in [0, 90, 270] {
        let (input, regions) = reg(
            content,
            rotate,
            &[[200.0, 80.0, 240.0, 120.0], [150.0, 310.0, 300.0, 330.0]],
        );
        let r = run(&format!("rotated{rotate}"), input, &regions);
        // The second region also crosses the red rectangle's corner region? no: only rect 2.
        assert_eq!(r.report.rects_cut, 2, "rotate {rotate}");
        assert!(r.report.collateral.is_empty(), "rotate {rotate}");
        r.assert_no_geometry_in_regions();
        r.assert_rest_unchanged();
    }
}

#[test]
fn lone_rectangles_are_cut_under_either_fill_rule_but_a_hole_goes_whole() {
    // blue: even-odd lone rectangle; green: nonzero lone rectangle (both cut). red and black
    // have a second rectangle over them (a hole): crossing -> removed whole, collateral.
    let content = "0 0 1 rg 50 100 100 100 re f*\n\
                   1 0 0 rg 160 100 100 100 re 180 120 60 60 re f*\n\
                   0 0.6 0 rg 20 250 100 100 re f\n\
                   0 0 0 rg 160 250 100 100 re 180 270 60 60 re f";
    let (input, regions) = reg(
        content,
        0,
        &[
            [100.0, 140.0, 140.0, 160.0],
            [150.0, 105.0, 250.0, 195.0],
            [100.0, 280.0, 140.0, 300.0],
            [150.0, 255.0, 250.0, 345.0],
        ],
    );
    let r = run("holes", input, &regions);
    assert_eq!(r.report.rects_cut, 2);
    // Each holed path: the outer rectangle crosses (collateral), the hole is inside.
    assert_eq!(r.report.subpaths_removed, 4);
    assert_eq!(r.report.collateral.len(), 2);
    r.assert_no_geometry_in_regions();
    r.assert_rest_unchanged();
}

#[test]
fn outlined_text_inside_a_region_goes_and_crossing_outlines_are_collateral() {
    // Three "glyphs" (outer + inner contour, even-odd): the first lies inside, the second
    // crosses the region edge, the third is far away.
    let mut content = String::from("0 0 0 rg\n");
    for cx in [60.0, 100.0, 220.0] {
        content += &format!("{}{}f*\n", circle(cx, 350.0, 15.0), circle(cx, 350.0, 6.0));
    }
    let (input, regions) = reg(&content, 0, &[[40.0, 330.0, 95.0, 370.0]]);
    let r = run("outlined", input, &regions);
    assert_eq!((r.report.rects_cut, r.report.subpaths_removed), (0, 4));
    assert_eq!(r.report.collateral.len(), 2);
    r.assert_no_geometry_in_regions();
    r.assert_rest_unchanged();
    r.assert_region_blank(0);
    // Only the third glyph's path (two contours) survives.
    assert_eq!(r.output_paths(), (0, 1));
}

#[test]
fn a_singular_transform_is_removed_not_cut() {
    // `1 0 0 0 100 300 cm` squashes the 100 x 100 rectangle onto the line y = 300.
    let content = "1 0 0 rg 20 20 60 60 re f\n\
                   q 1 0 0 0 100 300 cm 0 0 1 rg 0 0 100 100 re f Q";
    let (input, regions) = reg(content, 0, &[[120.0, 290.0, 160.0, 310.0]]);
    let r = run("singular", input, &regions);
    assert_eq!((r.report.rects_cut, r.report.subpaths_removed), (0, 1));
    assert_eq!(r.report.collateral.len(), 1);
    assert_eq!(r.output_paths(), (0, 1));
}

#[test]
fn clip_only_paths_stay_and_a_fully_removed_path_keeps_its_clip() {
    // 1: a clip-only path, then a page-sized fill under it: the fill is cut around the region
    //    and stays clipped. 2: `W f` whose rectangle lies wholly inside a region keeps `W n`;
    //    the magenta page-sized fill after it must stay confined by that clip (fidelity.py
    //    sees magenta everywhere outside the region if the clip were lost).
    let content = "q 100 200 80 80 re W n 1 0 0 rg 0 0 300 400 re f Q\n\
                   q 0 0 1 rg 200 50 60 60 re W f 1 0 1 rg 0 0 300 400 re f Q";
    let (input, regions) = reg(
        content,
        0,
        &[[120.0, 220.0, 160.0, 260.0], [190.0, 40.0, 270.0, 120.0]],
    );
    let before = Document::open(input.clone(), None)
        .unwrap()
        .page_model(0)
        .unwrap();
    // `W n` reports a clip item; the clip of `W f` only takes effect after the paint.
    assert_eq!(before.items.len(), 4, "a clip item and three fills");
    let r = run("clips", input, &regions);
    // Rect 1 and the magenta fill (both page-sized) are cut; the blue clip rectangle is inside.
    assert_eq!((r.report.rects_cut, r.report.subpaths_removed), (2, 1));
    // Both clips survive (the second now as `W n`); the removed fill is gone.
    assert_eq!(r.output_paths(), (2, 2));
    r.assert_no_geometry_in_regions();
    r.assert_rest_unchanged();
}

// ---- tiling patterns ------------------------------------------------------------------------------

#[test]
fn a_tiling_pattern_with_text_in_its_cell_under_a_region_is_refused() {
    let input = tiled(
        "/Pattern cs /P1 scn 50 100 200 100 re f",
        TEXT_CELL,
        PATH_CELL,
    );
    let doc = Document::open(input.clone(), None).unwrap();
    let regions = vec![region(&doc, 100.0, 120.0, 160.0, 160.0)];
    let e = refusal(input, &regions, Limits::default());
    assert_eq!(e.code, ErrorCode::RedactPatternUnsupported);
    assert_eq!(e.details.as_ref().unwrap().pages, Some(vec![0]));
}

#[test]
fn a_pattern_without_text_or_images_is_cut_and_a_text_pattern_elsewhere_is_ignored() {
    // P1 (text cell) paints far from the region; P2 (path cell) paints under it.
    let input = tiled(
        "/Pattern cs /P1 scn 200 300 50 50 re f /P2 scn 50 100 200 100 re f",
        TEXT_CELL,
        PATH_CELL,
    );
    let doc = Document::open(input.clone(), None).unwrap();
    let regions = vec![region(&doc, 100.0, 120.0, 160.0, 160.0)];
    let r = run("pattern-ok", input, &regions);
    assert_eq!((r.report.rects_cut, r.report.subpaths_removed), (1, 0));
    // get_drawings does not see pattern fills: judge the render instead.
    r.assert_region_blank(0);
    r.assert_rest_unchanged();
}

#[test]
fn a_huge_path_is_bounded_by_the_work_budget_before_anything_is_written() {
    let content: String = (0..40_000)
        .map(|i| format!("{} 10 2 2 re ", i % 250))
        .collect::<String>()
        + "f";
    let input = page(&content, 0);
    let doc = Document::open(input.clone(), None).unwrap();
    let regions = vec![region(&doc, 100.0, 5.0, 140.0, 30.0)];
    let limits = Limits {
        max_work_units: 20_000,
        ..Limits::default()
    };
    let e = refusal(input, &regions, limits);
    assert_eq!(e.code, ErrorCode::LimitExceeded);
}
