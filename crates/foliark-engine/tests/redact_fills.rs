//! Filled vector cuts (spec §5.4 rule 4, ADR 0009, NEW-86): a painted path is one unit, subtracted
//! from the regions with `i_overlay` on a 1/1024 pt grid; clip outlines are cut the same way. No
//! path is removed merely for crossing a region (no collateral). Judged on the OUTPUT by
//! vector_cut.py (decoded content, no clip trusted: vertices, containment and a clip-stripped
//! render), objects.py (PyMuPDF `get_drawings`), a tight 144 dpi fidelity.py with NO collateral
//! exclusion, and a PyMuPDF render of each region.
#[path = "vector_support/mod.rs"]
mod support;
use foliark_engine::Document;
use foliark_model::{ErrorCode, Limits};
use support::{PATH_CELL, Run, TEXT_CELL, circle, page, refusal, region, run, tiled};

fn reg(content: &str, rotate: i32, boxes: &[[f64; 4]]) -> (Vec<u8>, Vec<foliark_model::Rect>) {
    let input = page(content, rotate);
    let doc = Document::open(input.clone(), None).unwrap();
    let regions = boxes
        .iter()
        .map(|b| region(&doc, b[0], b[1], b[2], b[3]))
        .collect();
    (input, regions)
}

/// Run a case, then every independent check that applies to a cut fill.
fn cut(name: &str, content: &str, rotate: i32, boxes: &[[f64; 4]]) -> Run {
    let (input, regions) = reg(content, rotate, boxes);
    let r = run(name, input, &regions);
    r.assert_cut_is_precise();
    r.assert_rest_unchanged_tight();
    r
}

/// A circle of four Beziers running the other way round (a hole under the nonzero rule).
fn circle_rev(cx: f64, cy: f64, r: f64) -> String {
    let k = 0.5523 * r;
    format!(
        "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h ",
        cx + r,
        cy,
        cx + r,
        cy - k,
        cx + k,
        cy - r,
        cx,
        cy - r,
        cx - k,
        cy - r,
        cx - r,
        cy - k,
        cx - r,
        cy,
        cx - r,
        cy + k,
        cx - k,
        cy + r,
        cx,
        cy + r,
        cx + k,
        cy + r,
        cx + r,
        cy + k,
        cx + r,
        cy
    )
}

/// A five-pointed star as one self-intersecting contour (every second point).
fn star(cx: f64, cy: f64, r: f64) -> String {
    let p = |k: usize| {
        let a = std::f64::consts::FRAC_PI_2 + k as f64 * 2.0 * std::f64::consts::PI / 5.0;
        (cx + r * a.cos(), cy + r * a.sin())
    };
    let pts: Vec<String> = [0, 2, 4, 1, 3]
        .iter()
        .enumerate()
        .map(|(n, &k)| format!("{} {} {}", p(k).0, p(k).1, if n == 0 { "m" } else { "l" }))
        .collect();
    pts.join(" ") + " h "
}

// ---- cuts ---------------------------------------------------------------------------------------

#[test]
fn partial_overlap_cuts_the_crossing_rectangle_and_leaves_its_neighbours_byte_for_byte() {
    // Region 1 sits inside the blue rectangle (a frame remains); region 2 crosses the right edge
    // of the first red rectangle, the second red rectangle shares its path and must keep its
    // operators; the green one is far away.
    let content = "0 0 1 rg 50 100 200 100 re f\n\
                   1 0 0 rg 20 320 60 40 re 200 320 60 40 re f\n\
                   0 0.6 0 rg 20 20 260 30 re f";
    let r = cut(
        "partial",
        content,
        0,
        &[[100.0, 140.0, 160.0, 180.0], [60.0, 330.0, 100.0, 350.0]],
    );
    assert_eq!(
        (
            r.report.paths_cut,
            r.report.subpaths_removed,
            r.report.approximated
        ),
        (2, 0, 0)
    );
    let out = r.content(true);
    assert!(out.contains("200 320 60 40 re"), "{out}");
    assert!(out.contains("20 20 260 30 re"), "{out}");
    assert!(
        !out.contains("20 320 60 40 re"),
        "the crossing rectangle is replaced: {out}"
    );
    r.assert_no_geometry_in_regions();
    r.assert_region_blank(0);
    r.assert_region_blank(1);
}

#[test]
fn a_curved_path_is_cut_where_it_crosses_and_everything_else_of_it_stays() {
    // A circle (four Beziers) and a mirrored 2 x 0.5 scaled copy: curves become 0.01 pt polylines
    // only on the cut paths, and the outline outside the regions renders alike.
    let content = format!(
        "0 0 1 rg {}f\nq 2 0 0 -0.5 10 390 cm 0 0.5 0 rg {}f Q",
        circle(150.0, 200.0, 60.0),
        circle(100.0, 100.0, 40.0)
    );
    // Region 1 straddles the circle's right edge, region 2 its top; region 3 the squashed one.
    let r = cut(
        "curves",
        &content,
        0,
        &[
            [190.0, 180.0, 230.0, 220.0],
            [120.0, 240.0, 180.0, 270.0],
            [150.0, 325.0, 200.0, 345.0],
        ],
    );
    assert_eq!((r.report.paths_cut, r.report.approximated), (2, 2));
    r.assert_no_geometry_in_regions();
    r.assert_region_blank(0);
    r.assert_region_blank(2);
}

#[test]
fn a_ring_keeps_its_hole_and_a_region_over_the_hole_leaves_the_path_untouched() {
    // Nonzero ring: outer circle one way, inner the other. Region 1 lies in the hole only: the
    // ink never meets it, so the stream is not rewritten. Region 2 crosses the ring wall.
    let content = format!(
        "0.8 0 0 rg {}{}f",
        circle(150.0, 200.0, 60.0),
        circle_rev(150.0, 200.0, 25.0)
    );
    let (input, regions) = reg(&content, 0, &[[140.0, 190.0, 160.0, 210.0]]);
    let r = run("ring-hole", input, &regions);
    assert_eq!(
        (
            r.report.paths_cut,
            r.report.subpaths_removed,
            r.report.untouched
        ),
        (0, 0, 1)
    );
    assert_eq!(r.content(true), r.content(false), "byte for byte");
    r.assert_rest_unchanged_tight();

    let r = cut("ring-wall", &content, 0, &[[80.0, 190.0, 160.0, 210.0]]);
    assert_eq!(r.report.paths_cut, 1);
    r.assert_region_blank(0);
}

#[test]
fn even_odd_and_nonzero_stars_keep_their_own_winding_outside_the_cut() {
    // Even-odd star: its centre pentagon is a hole, cut one tip. Nonzero star: the centre is
    // inked, cut a square out of that centre (the arms stay, the hole is only the cut).
    let content = format!(
        "0 0 0.6 rg {}f*\n0.6 0 0 rg {}f",
        star(90.0, 280.0, 70.0),
        star(210.0, 100.0, 70.0)
    );
    let r = cut(
        "stars",
        &content,
        0,
        &[[80.0, 335.0, 100.0, 355.0], [200.0, 90.0, 220.0, 110.0]],
    );
    assert_eq!(r.report.paths_cut, 2);
    r.assert_no_geometry_in_regions();
    r.assert_region_blank(0);
    r.assert_region_blank(1);
}

#[test]
fn overlapping_regions_remove_their_union_even_under_even_odd() {
    // Two regions overlap over x 140..160: subtracted one by one under even-odd they would
    // toggle the overlap back in.
    let content = format!("0 0 1 rg {}f*", circle(150.0, 200.0, 60.0));
    let r = cut(
        "union",
        &content,
        0,
        &[[110.0, 170.0, 160.0, 230.0], [140.0, 180.0, 200.0, 220.0]],
    );
    assert_eq!(r.report.paths_cut, 1);
    r.assert_region_blank(0);
    r.assert_region_blank(1);
}

#[test]
fn cuts_are_exact_under_rotation_flip_nonuniform_scale_and_page_rotate() {
    // Rotated 90 degrees, then y-flipped and scaled 2 x 0.5, then a 30 degree turn with a
    // triangle. The cut is written in the stream's own space; any error shows as a pixel
    // difference outside the regions.
    let content = "q 0 1 -1 0 300 0 cm 1 0 0 rg 20 20 150 100 re f Q\n\
                   q 2 0 0 -0.5 10 390 cm 0 0.5 0 rg 20 100 100 80 re f Q\n\
                   q 0.866 0.5 -0.5 0.866 150 40 cm 0 0 1 rg -40 -10 m 40 -10 l 0 50 l h f Q";
    for rotate in [0, 90, 270] {
        let r = cut(
            &format!("rotated{rotate}"),
            content,
            rotate,
            &[
                [200.0, 80.0, 240.0, 120.0],
                [150.0, 310.0, 300.0, 330.0],
                [140.0, 40.0, 165.0, 70.0],
            ],
        );
        assert_eq!(r.report.paths_cut, 3, "rotate {rotate}");
        assert_eq!(r.report.subpaths_removed, 0, "rotate {rotate}");
        r.assert_no_geometry_in_regions();
    }
}

#[test]
fn a_path_hidden_by_a_clip_is_still_removed_inside_a_region() {
    // The red square is wholly clipped away yet sits inside the region: its geometry would
    // still be in the file. The clip itself is far away and stays.
    let content = "q 0 0 50 50 re W n 1 0 0 rg 100 100 40 40 re f Q\n0 0 1 rg 200 300 60 40 re f";
    let (input, regions) = reg(content, 0, &[[90.0, 90.0, 150.0, 150.0]]);
    let r = run("hidden", input, &regions);
    assert_eq!((r.report.subpaths_removed, r.report.paths_cut), (1, 0));
    r.assert_cut_is_precise();
    r.assert_rest_unchanged_tight();
    assert_eq!(r.output_paths(), (1, 1), "the far clip and rectangle stay");
}

#[test]
fn outlined_text_inside_a_region_goes_and_a_crossing_glyph_is_cut_not_removed() {
    // Three "glyphs" (outer + inner contour, even-odd): the first lies inside the region, the
    // second crosses its edge, the third is far away.
    let mut content = String::from("0 0 0 rg\n");
    for cx in [60.0, 100.0, 220.0] {
        content += &format!("{}{}f*\n", circle(cx, 350.0, 15.0), circle(cx, 350.0, 6.0));
    }
    let r = cut("outlined", &content, 0, &[[40.0, 330.0, 95.0, 370.0]]);
    assert_eq!((r.report.subpaths_removed, r.report.paths_cut), (1, 1));
    r.assert_no_geometry_in_regions();
    r.assert_region_blank(0);
    // The crossing glyph (cut) and the far one remain.
    assert_eq!(r.output_paths(), (0, 2));
}

#[test]
fn a_singular_transform_is_removed_not_cut() {
    // `1 0 0 0 100 300 cm` squashes the 100 x 100 rectangle onto the line y = 300.
    let content = "1 0 0 rg 20 20 60 60 re f\n\
                   q 1 0 0 0 100 300 cm 0 0 1 rg 0 0 100 100 re f Q";
    let (input, regions) = reg(content, 0, &[[120.0, 290.0, 160.0, 310.0]]);
    let r = run("singular", input, &regions);
    assert_eq!((r.report.paths_cut, r.report.subpaths_removed), (0, 1));
    assert_eq!(r.output_paths(), (0, 1));
}

// ---- clips are geometry -------------------------------------------------------------------------

#[test]
fn a_clip_that_only_encloses_a_region_is_left_alone() {
    // The clip rectangle holds the region in its area but its outline never meets it: the clip
    // is public frame, not protected geometry. The page-sized fill under it is cut.
    let content = "q 100 200 80 80 re W n 1 0 0 rg 0 0 300 400 re f Q";
    let r = cut("clip-encloses", content, 0, &[[120.0, 220.0, 160.0, 260.0]]);
    let out = r.content(true);
    assert!(out.contains("100 200 80 80 re"), "{out}");
    assert_eq!(r.report.paths_cut, 1);
}

#[test]
fn a_clip_only_outline_in_a_region_is_replaced_by_its_difference() {
    // The region covers the clip's corner (180, 200) and crosses its right edge: the original
    // outline would still be in the file, so it is cut. The page-sized fill under the clip is cut
    // too and the visible result outside the region is unchanged.
    let content = "q 100 200 80 80 re W n 1 0 0 rg 0 0 300 400 re f Q";
    let r = cut("clip-only", content, 0, &[[170.0, 190.0, 200.0, 215.0]]);
    assert_eq!(r.report.paths_cut, 2, "the clip and the fill");
    assert!(!r.content(true).contains("100 200 80 80 re"));
    // Both survive, as an L-shaped clip and the cut fill.
    assert_eq!(r.output_paths(), (1, 1));
}

#[test]
fn a_painted_clip_is_cut_after_its_paint_and_still_confines_what_follows() {
    // `W f`: the rectangle (200..260 x 50..110) is painted and clips. The region covers its
    // corner. The magenta page fill after it only shows inside the clip.
    let content = "q 200 50 60 60 re W f 1 0 1 rg 0 0 300 400 re f Q";
    let r = cut("clip-painted", content, 0, &[[190.0, 40.0, 230.0, 90.0]]);
    assert!(!r.content(true).contains("200 50 60 60 re"));
    assert_eq!(r.report.paths_cut, 2);
}

#[test]
fn a_clip_wholly_inside_a_region_becomes_an_empty_clip() {
    // Everything the clip let through lies inside the region, which is removed anyway; the
    // magenta fill after it stays confined (to nothing).
    let content = "q 200 50 60 60 re W f 1 0 1 rg 0 0 300 400 re f Q";
    let (input, regions) = reg(content, 0, &[[190.0, 40.0, 270.0, 120.0]]);
    let r = run("clip-empty", input, &regions);
    assert_eq!(r.report.subpaths_removed, 1);
    let out = r.content(true);
    assert!(out.contains("0 0 0 0 re"), "{out}");
    assert!(!out.contains("200 50 60 60 re"), "{out}");
    r.assert_cut_is_precise();
    r.assert_rest_unchanged_tight();
}

#[test]
fn a_nested_clip_stays_and_the_geometry_under_it_is_cut() {
    let content = "q 40 40 220 320 re W n q 60 60 180 280 re W n 0 0 1 rg 20 100 260 100 re f Q Q";
    let r = cut("nested", content, 0, &[[100.0, 140.0, 160.0, 180.0]]);
    let out = r.content(true);
    assert!(
        out.contains("40 40 220 320 re") && out.contains("60 60 180 280 re"),
        "{out}"
    );
    assert_eq!(r.report.paths_cut, 1);
    assert_eq!(
        r.report.untouched, 2,
        "two clips whose outlines miss the region"
    );
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
    assert_eq!((r.report.paths_cut, r.report.subpaths_removed), (1, 0));
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

// ---- caps ---------------------------------------------------------------------------------------

/// A saw polygon of `n` vertices over x 0..260, y 100..105 (one subpath, one huge path).
fn saw(n: usize) -> String {
    let mut s = String::from("0 0 1 rg 0 100 m ");
    for i in 1..n {
        s += &format!("{} {} l ", i as f64 * 260.0 / n as f64, 100 + 5 * (i % 2));
    }
    s + "f"
}

fn limit_of(input: Vec<u8>, regions: &[foliark_model::Rect]) -> String {
    let e = refusal(input, regions, Limits::default());
    assert_eq!(e.code, ErrorCode::LimitExceeded);
    e.details.unwrap().limit.unwrap()
}

#[test]
fn a_path_over_the_vertex_cap_is_refused_before_any_boolean_runs() {
    let (input, regions) = reg(&saw(260_000), 0, &[[100.0, 90.0, 140.0, 120.0]]);
    assert_eq!(limit_of(input, &regions), "vectorVertices");
}

#[test]
fn coordinates_over_the_cap_are_refused() {
    let content = "0 0 1 rg -1000000 -1000000 2000000 2000000 re f";
    let (input, regions) = reg(content, 0, &[[100.0, 100.0, 140.0, 140.0]]);
    assert_eq!(limit_of(input, &regions), "vectorCoordinate");
}

#[test]
#[ignore = "timing at the vertex cap: cargo test --release -p foliark-engine --test redact_fills -- --ignored --nocapture"]
fn a_path_just_under_the_vertex_cap_is_cut_and_its_time_recorded() {
    let (input, regions) = reg(&saw(249_000), 0, &[[100.0, 90.0, 140.0, 120.0]]);
    let t = std::time::Instant::now();
    let r = run("cap", input, &regions);
    println!("redact_vectors at 249000 vertices: {:?}", t.elapsed());
    assert_eq!(r.report.paths_cut, 1);
}

#[test]
fn a_path_clear_of_every_region_keeps_the_stream_byte_for_byte() {
    let content = "0 0 1 rg 50 100 100 100 re f\n1 0 0 rg 200 300 40 40 re f";
    let (input, regions) = reg(content, 0, &[[10.0, 10.0, 30.0, 30.0]]);
    let r = run("far", input, &regions);
    assert_eq!(r.content(true), r.content(false));
}

#[test]
fn the_output_check_finds_ink_and_clip_outline_left_in_a_region_and_accepts_the_cut() {
    // Spec §5.7: the written page is re-planned against the regions and every filled path and
    // clip outline judged on its geometry. The input fails it, the cut output passes it.
    use foliark_engine::redaction::vector::page_leaks;
    let content = "q 100 200 80 80 re W n 0 0 1 rg 50 150 200 100 re f Q";
    let (input, regions) = reg(content, 0, &[[170.0, 190.0, 200.0, 215.0]]);
    let leaks = |bytes: Vec<u8>| {
        let doc = Document::open(bytes, None).unwrap();
        let shrunk: Vec<_> = regions
            .iter()
            .map(|r| foliark_model::Rect {
                x: r.x + 0.01,
                y: r.y + 0.01,
                width: r.width - 0.02,
                height: r.height - 0.02,
            })
            .collect();
        let plan = support::plan_for(&doc, &shrunk);
        page_leaks(&doc, &plan.pages[0], &|_| false).unwrap()
    };
    assert!(
        leaks(input.clone()),
        "the input still has ink in the region"
    );
    let r = run("verify", input, &regions);
    assert!(!leaks(r.output.clone()), "the cut output has none");
}
