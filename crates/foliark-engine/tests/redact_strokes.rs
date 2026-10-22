//! Stroked vector cuts (spec §5.4 rule 4, ADR 0009, NEW-87): a stroke is expanded by kurbo in its
//! own space (width, caps, joins, miter limit, dashes, non-uniform CTM), subtracted from the
//! regions and written back as a filled outline in the stroking colour; subpaths clear of every
//! region keep their operators. No stroke is removed merely for crossing a region. Judged on the
//! OUTPUT by vector_cut.py (decoded content, no clip trusted: centreline reach and a clip-stripped
//! render), a PyMuPDF render of each region, and a tight 144 dpi fidelity.py with NO collateral
//! exclusion (every stroke segment outside the regions must render alike).
#[path = "vector_support/mod.rs"]
mod support;
use foliark_engine::Document;
use foliark_engine::redaction::vector::redact_vectors;
use foliark_engine::writer::Writer;
use foliark_model::{ErrorCode, Limits};
use std::sync::atomic::Ordering;
use support::{PATH_CELL, Run, TEXT_CELL, assemble, page, refusal, region, run, tiled};

fn reg(content: &str, rotate: i32, boxes: &[[f64; 4]]) -> (Vec<u8>, Vec<foliark_model::Rect>) {
    regions_of(page(content, rotate), boxes)
}

fn regions_of(input: Vec<u8>, boxes: &[[f64; 4]]) -> (Vec<u8>, Vec<foliark_model::Rect>) {
    let doc = Document::open(input.clone(), None).unwrap();
    let regions = boxes
        .iter()
        .map(|b| region(&doc, b[0], b[1], b[2], b[3]))
        .collect();
    (input, regions)
}

/// Run a case, then every independent check that applies to a cut stroke.
fn cut_in(name: &str, input: Vec<u8>, rotate: i32, boxes: &[[f64; 4]]) -> Run {
    let (input, regions) = regions_of(input, boxes);
    let r = run(name, input, &regions);
    r.assert_cut_is_precise();
    r.assert_rest_unchanged_tight();
    if rotate == 0 {
        for n in 0..boxes.len() {
            r.assert_region_blank(n);
        }
    }
    r
}

fn cut(name: &str, content: &str, rotate: i32, boxes: &[[f64; 4]]) -> Run {
    cut_in(name, page(content, rotate), rotate, boxes)
}

/// `(removed, cut, untouched)` of the report.
fn counts(r: &Run) -> (u32, u32, u32) {
    let v = &r.report;
    (v.subpaths_removed, v.paths_cut, v.untouched)
}

#[test]
fn a_straight_stroke_keeps_both_ends_and_its_colour() {
    let r = cut(
        "straight",
        "6 w 0 0.2 1 RG 20 200 m 280 200 l S\n1 w 0 0 0 RG 20 50 m 280 60 l S",
        0,
        &[[100.0, 190.0, 140.0, 210.0]],
    );
    assert_eq!(counts(&r), (0, 1, 0));
    assert_eq!(
        r.report.approximated, 1,
        "a stroke that became a fill is disclosed"
    );
    // Cut outline in the stroking colour as a fill, inside q..Q; the far stroke is byte-kept.
    let out = r.content(true);
    assert!(out.contains("q 0 0.2 1 rg"), "{out}");
    assert!(out.contains("20 50 m 280 60 l S"), "{out}");
    assert!(
        !out.contains("20 200 m 280 200 l"),
        "the original stroke path is gone: {out}"
    );
    assert_eq!(r.output_paths(), (0, 2));
}

#[test]
fn a_page_stroke_with_no_colour_operator_paints_the_initial_black() {
    let r = cut(
        "initial-black",
        "6 w 20 200 m 280 200 l S",
        0,
        &[[100.0, 190.0, 140.0, 210.0]],
    );
    assert!(r.content(true).contains("q 0 g"), "{}", r.content(true));
}

#[test]
fn a_curved_stroke_crossing_the_boundary_is_cut_not_removed() {
    let r = cut(
        "curve",
        "5 w 1 0 0 RG 40 340 m 100 380 200 300 260 350 c S",
        0,
        &[[120.0, 330.0, 160.0, 360.0]],
    );
    assert_eq!(counts(&r), (0, 1, 0));
}

#[test]
fn a_thick_mitered_corner_reaching_into_the_region_is_cut_at_the_tip() {
    // 8 pt wide, 30 degree corner at (200, 305): the miter tip lies 15.5 pt past the vertex,
    // inside the region, while the path's own box ends at x = 200.
    let r = cut(
        "miter",
        "8 w 10 M 0 0 0 RG 100 331.8 m 200 305 l 100 278.2 l S\n2 w 20 20 m 60 40 l S",
        0,
        &[[205.0, 295.0, 230.0, 315.0]],
    );
    assert_eq!(counts(&r), (0, 1, 0));
    assert_eq!(r.output_paths(), (0, 2), "the far stroke stays");
}

#[test]
fn caps_decide_whether_a_stroke_end_reaches_the_region() {
    // The line ends at x = 100; the region starts at 102. A butt cap stops at 100 (byte-exact,
    // nothing rewritten); round and square caps extend 5 pt, into the region, and are cut.
    let region = [[102.0, 90.0, 130.0, 110.0]];
    for (cap, cut_expected) in [(0, false), (1, true), (2, true)] {
        let content = format!("10 w {cap} J 0 0 0 RG 50 100 m 100 100 l S");
        if cut_expected {
            let r = cut(&format!("cap{cap}"), &content, 0, &region);
            assert_eq!(counts(&r), (0, 1, 0), "cap {cap}");
        } else {
            let (input, regions) = reg(&content, 0, &region);
            let r = run("cap0", input, &regions);
            assert_eq!(counts(&r), (0, 0, 1));
            assert_eq!(
                r.content(true),
                r.content(false),
                "a butt cap never reaches"
            );
            r.assert_rest_unchanged_tight();
        }
    }
}

#[test]
fn a_square_cap_corner_past_the_miter_factor_is_still_inside_the_reach() {
    // Miter limit 1 and a 20 pt square-capped line at 45 degrees: the cap corner is 14.1 pt
    // from the end point, the miter factor alone would give 10 pt.
    let r = cut(
        "square-corner",
        "20 w 1 M 2 J 0 0 0 RG 50 50 m 150 150 l S",
        0,
        &[[161.0, 140.0, 175.0, 155.0]],
    );
    assert_eq!(counts(&r), (0, 1, 0));
}

#[test]
fn a_dashed_curve_keeps_its_dashes_outside_the_region() {
    // The curve passes (150, 341) at t = 0.5; dashes elsewhere keep their shape.
    let r = cut(
        "dashed",
        "3 w [6 3] 0 d 0 0 1 RG 40 340 m 100 380 200 300 260 350 c S\n[] 0 d 1 w 0 0 0 RG 20 20 m 280 30 l S",
        0,
        &[[120.0, 330.0, 160.0, 360.0]],
    );
    assert_eq!(counts(&r), (0, 1, 0));
}

#[test]
fn a_non_uniform_scale_cuts_the_stretched_stroke_in_its_own_space() {
    // x is stretched 10 times: the 1 pt line is 10 pt wide across x = 195..205; the region
    // starts at 202, so the left 7 pt of the ribbon must survive (a display-space stroke or a
    // uniform-scale reach would get this wrong).
    let r = cut(
        "stretched",
        "q 10 0 0 1 0 0 cm 1 w 1 M 0 0 0 RG 20 100 m 20 200 l S Q",
        0,
        &[[202.0, 130.0, 215.0, 170.0]],
    );
    assert_eq!(counts(&r), (0, 1, 0));
    assert!(
        r.content(true).contains("rg"),
        "colour mirrored inside the cm"
    );
}

#[test]
fn fill_and_stroke_are_cut_each_by_its_own_rule() {
    // `B`: the blue fill and the red 6 pt border both cross the region; the green fill-only
    // rectangle in region 1 is cut as a fill.
    let r = cut(
        "fill-stroke",
        "0 0 1 rg 1 0 0 RG 6 w 50 100 100 100 re B\n0 0.6 0 rg 160 250 100 60 re f",
        0,
        &[[120.0, 140.0, 170.0, 160.0], [200.0, 270.0, 230.0, 290.0]],
    );
    assert_eq!(counts(&r), (0, 3, 0));
    let out = r.content(true);
    assert!(out.contains("q 1 0 0 rg"), "{out}");
    assert!(
        !out.contains(" B "),
        "B is written as its two halves: {out}"
    );
}

#[test]
fn a_closed_stroke_with_an_untouched_fill_keeps_the_fill_bytes() {
    // `b` closes, fills and strokes: the fill is clear of the region (the border is not).
    let r = cut(
        "closed-b",
        "0 0 1 rg 1 0 0 RG 10 w 50 100 m 150 100 l 100 200 l b",
        0,
        &[[140.0, 95.0, 160.0, 105.0]],
    );
    assert_eq!(r.report.paths_cut, 2);
}

#[test]
fn a_rotated_page_and_a_rotated_cm_cut_the_same_stroke() {
    for rotate in [90, 270] {
        let r = cut(
            &format!("rot{rotate}"),
            "q 0.866 0.5 -0.5 0.866 80 20 cm 5 w 0 0.5 0 RG 0 100 m 200 100 l S Q",
            rotate,
            &[[150.0, 160.0, 190.0, 200.0]],
        );
        assert_eq!(counts(&r), (0, 1, 0), "rotate {rotate}");
    }
}

fn form_page(page_content: &str, form_content: &str) -> Vec<u8> {
    let stream = |dict: &str, data: &str| {
        format!(
            "<< {dict} /Length {} >>\nstream\n{data}\nendstream",
            data.len()
        )
    };
    assemble(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /XObject << /Fm1 5 0 R >> >> >>",
        &stream("", page_content),
        &stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Matrix [0 1 -1 0 300 0]",
            form_content,
        ),
    ])
}

#[test]
fn a_stroke_inside_a_transformed_form_is_cut_in_the_form() {
    // The form matrix turns the stream a quarter turn; its stroke colour is its own.
    let input = form_page("/Fm1 Do", "0.5 0 0.5 RG 6 w 40 100 m 40 260 l S");
    let r = cut_in("form", input, 0, &[[100.0, 30.0, 140.0, 50.0]]);
    assert_eq!(counts(&r), (0, 1, 0));
}

#[test]
fn a_stroke_colour_inherited_from_outside_the_stream_is_refused() {
    let input = form_page("1 0 0 RG /Fm1 Do", "6 w 40 100 m 40 260 l S");
    let (input, regions) = regions_of(input, &[[100.0, 30.0, 140.0, 50.0]]);
    let e = refusal(input, &regions, Limits::default());
    assert_eq!(e.code, ErrorCode::RedactVectorUnsupported);
    let d = e.details.unwrap();
    assert_eq!(
        (d.feature.as_deref(), d.pages),
        (Some("strokeColorInherited"), Some(vec![0]))
    );
}

fn gs_page(content: &str) -> Vec<u8> {
    let stream = format!(
        "<< /Length {} >>\nstream\n{content}\nendstream",
        content.len()
    );
    assemble(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /ExtGState << /Same << /CA 0.5 /ca 0.5 >> /Diff << /CA 0.5 /ca 1 >> >> >> >>",
        &stream,
    ])
}

#[test]
fn stroke_alpha_must_equal_fill_alpha_and_a_b_may_not_be_transparent() {
    let region = [[120.0, 190.0, 160.0, 210.0]];
    let feature = |content: &str| {
        let (input, regions) = regions_of(gs_page(content), &region);
        let e = refusal(input, &regions, Limits::default());
        assert_eq!(e.code, ErrorCode::RedactVectorUnsupported);
        e.details.unwrap().feature.unwrap()
    };
    assert_eq!(
        feature("/Diff gs 0 0 0 RG 6 w 20 200 m 280 200 l S"),
        "strokeAlphaDiffers"
    );
    assert_eq!(
        feature("/Same gs 0 0 1 rg 0 0 0 RG 6 w 100 150 100 100 re B"),
        "strokeBothAlpha"
    );
    // Equal alphas on a lone stroke reproduce exactly as a fill.
    cut_in(
        "same-alpha",
        gs_page("/Same gs 0 0 0 RG 6 w 20 200 m 280 200 l S"),
        0,
        &region,
    );
}

#[test]
fn a_stroke_painted_by_a_pattern_is_refused_not_guessed() {
    let input = tiled(
        "/Pattern CS /P1 SCN 8 w 50 100 m 250 100 l S",
        PATH_CELL,
        PATH_CELL,
    );
    let (input, regions) = regions_of(input, &[[100.0, 90.0, 160.0, 110.0]]);
    let e = refusal(input, &regions, Limits::default());
    assert_eq!(e.code, ErrorCode::RedactVectorUnsupported);
    assert_eq!(e.details.unwrap().feature.as_deref(), Some("strokePattern"));
}

#[test]
fn stroke_paint_from_a_tiling_pattern_with_text_is_still_refused_as_a_pattern() {
    let input = tiled(
        "/Pattern CS /P1 SCN 8 w 50 100 m 250 100 l S",
        TEXT_CELL,
        PATH_CELL,
    );
    let (input, regions) = regions_of(input, &[[100.0, 90.0, 160.0, 110.0]]);
    let e = refusal(input, &regions, Limits::default());
    assert_eq!(e.code, ErrorCode::RedactPatternUnsupported);
}

#[test]
fn table_rules_cut_only_the_rule_that_crosses() {
    // 40 horizontal 0.5 pt rules, one region over two of them: the other 38 stay `S`, byte for byte.
    let content: String = (0..40)
        .map(|i| {
            format!(
                "0.5 w 0 0 0 RG 20 {} m 280 {} l S\n",
                20 + i * 9,
                20 + i * 9
            )
        })
        .collect();
    let r = cut("rules", &content, 0, &[[100.0, 100.0, 140.0, 112.0]]);
    assert_eq!((r.report.paths_cut, r.report.untouched), (2, 0));
    assert_eq!(r.output_paths(), (0, 40));
}

#[test]
fn an_inside_stroke_is_removed_and_a_far_one_stays() {
    let (input, regions) = reg(
        "2 w 100 100 m 120 120 l S\n250 350 m 280 380 l S",
        0,
        &[[90.0, 90.0, 130.0, 130.0]],
    );
    let r = run("inside", input, &regions);
    assert_eq!(counts(&r), (1, 0, 0));
    r.assert_region_blank(0);
    r.assert_rest_unchanged_tight();
    assert_eq!(r.output_paths(), (0, 1));
}

#[test]
fn a_hairline_is_cut_and_leaves_nothing_in_the_region() {
    // Width 0: drawn at the renderer's minimum, cut at 0.01 pt (disclosed), so only the region
    // and the geometry are judged here, not a hairline's rendered width elsewhere.
    let (input, regions) = reg(
        "0 w 0 0 0 RG 20 200 m 280 200 l S",
        0,
        &[[100.0, 190.0, 140.0, 210.0]],
    );
    let r = run("hairline", input, &regions);
    assert_eq!(counts(&r), (0, 1, 0));
    r.assert_cut_is_precise();
    r.assert_region_blank(0);
}

#[test]
fn a_singular_transform_stroke_is_removed_not_kept() {
    // `1 0 0 0 100 300 cm` collapses the line onto y = 300: no area, nothing recoverable.
    let (input, regions) = reg(
        "q 1 0 0 0 100 300 cm 4 w 0 0 m 100 0 l S Q",
        0,
        &[[120.0, 295.0, 160.0, 305.0]],
    );
    let r = run("singular-stroke", input, &regions);
    assert_eq!(counts(&r), (1, 0, 0));
    assert_eq!(r.output_paths(), (0, 0));
}

#[test]
fn a_stroke_clip_is_not_concealment() {
    // The clip hides the first stroke on screen (the input render of the region is blank), but
    // the stroke is geometry in the region: vector_cut.py renders with every clip removed.
    let (input, regions) = reg(
        "q 0 0 50 50 re W n 4 w 0 0 0 RG 20 300 m 280 300 l S Q\n0 0 0 RG 4 w 20 200 m 280 200 l S",
        0,
        &[[100.0, 290.0, 140.0, 310.0]],
    );
    let r = run("clipped", input, &regions);
    assert_eq!(counts(&r).0 + counts(&r).1, 1);
    r.assert_cut_is_precise();
    assert_eq!(r.ink(0), (0, 0));
}

// ---- bounds, cancellation ---------------------------------------------------------------------

fn limit_of(content: &str, limits: Limits) -> Option<String> {
    let input = page(content, 0);
    let doc = Document::open(input.clone(), None).unwrap();
    let regions = vec![region(&doc, 100.0, 5.0, 140.0, 30.0)];
    let e = refusal(input, &regions, limits);
    assert_eq!(e.code, ErrorCode::LimitExceeded);
    e.details.unwrap().limit
}

#[test]
fn a_dash_explosion_is_refused_before_kurbo_runs() {
    let l = limit_of(
        "0.1 w [0.01 0.01] 0 d 0 10 m 100000 10 l S",
        Limits::default(),
    );
    assert_eq!(l.as_deref(), Some("vectorDashes"));
}

fn zigzag(n: usize) -> String {
    let mut c = String::from("0.5 w 0 10 m ");
    for i in 0..n {
        c += &format!("{} {} l ", (i % 280) as f64 * 0.5 + 10.0, 10 + (i % 2) * 15);
    }
    c + "S"
}

#[test]
fn a_stroke_over_the_vertex_cap_is_refused() {
    // 130,000 zig-zag vertices: the outline has two sides, so over 250,000 flattened vertices.
    assert_eq!(
        limit_of(&zigzag(130_000), Limits::default()).as_deref(),
        Some("vectorVertices")
    );
}

#[test]
fn a_huge_stroked_path_is_bounded_by_the_work_budget() {
    let content: String = (0..40_000)
        .map(|i| format!("{} 10 m {} 12 l ", i % 250, i % 250))
        .collect::<String>()
        + "S";
    let limits = Limits {
        max_work_units: 20_000,
        ..Limits::default()
    };
    assert_eq!(limit_of(&content, limits).as_deref(), Some("maxWorkUnits"));
}

#[test]
fn a_cancelled_document_stops_the_stroke_cut_before_any_byte_is_written() {
    let (input, regions) = reg(
        "6 w 0 0 0 RG 20 200 m 280 200 l S",
        0,
        &[[100.0, 190.0, 140.0, 210.0]],
    );
    let doc = Document::open(input, None).unwrap();
    let plan = support::plan_for(&doc, &regions);
    doc.cancel_handle().store(true, Ordering::Relaxed);
    let mut w = Writer::new(doc.reader(), Limits::default());
    let e = redact_vectors(&doc, &plan, &mut w).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
    doc.cancel_handle().store(false, Ordering::Relaxed);
    let again = redact_vectors(&doc, &plan, &mut w).expect("runs once the flag is clear");
    assert_eq!(again.paths_cut, 1);
}

/// Release-build timing of one subpath near the vertex cap: `cargo test --release -- --ignored`.
#[test]
#[ignore = "timing probe, run in release"]
fn a_stroke_just_under_the_vertex_cap_is_cut_in_bounded_time() {
    let (input, regions) = reg(&zigzag(60_000), 0, &[[100.0, 5.0, 140.0, 30.0]]);
    let t = std::time::Instant::now();
    let r = run("near-cap", input, &regions);
    eprintln!(
        "60k-vertex stroke (about 240k ink vertices), whole redact_vectors: {:?}",
        t.elapsed()
    );
    assert_eq!(r.report.paths_cut, 1);
}
