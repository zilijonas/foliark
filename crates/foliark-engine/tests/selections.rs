//! Hit testing and selections (spec §3.5): one quad per visual line, whole-cluster snapping,
//! rotated text, columns.
mod common;
use common::{Font, build, close, width};
use foliark_model::Point;

const P: fn(f64, f64) -> Point = |x, y| Point { x, y };

/// Display y of a baseline at user y: the page is 400 high.
fn dy(user_y: f64) -> f64 {
    400.0 - user_y
}

#[test]
fn hit_test_prefers_the_containing_cluster_then_nearest_within_tolerance() {
    // "ab cd": 10 pt, 5 pt per glyph from x=20; baseline user y 300 (display 100).
    let doc = build(
        &["BT /F1 10 Tf 1 0 0 1 20 300 Tm (ab cd) Tj ET"],
        &Font::default(),
        0,
    );
    let layout = doc.text_layout(0, false).unwrap();
    let hit = layout.hit_test(P(27.0, dy(300.0) - 3.0), 2.0).unwrap();
    // x 27 is inside "b" (offsets 1..2), which is word 0.
    assert_eq!((hit.start, hit.end, hit.word_index), (1, 2, 0));
    // "c" is glyph 3 at x 35..40.
    assert_eq!(
        layout
            .hit_test(P(37.0, dy(300.0) - 3.0), 0.0)
            .unwrap()
            .start,
        3
    );
    // 1.5 pt above the glyph box top (ascent 0.8 em = 8 pt): inside tolerance 2, outside 1.
    let above = P(22.0, dy(300.0) - 9.5);
    assert_eq!(layout.hit_test(above, 2.0).unwrap().start, 0);
    assert!(layout.hit_test(above, 1.0).is_none());
}

#[test]
fn select_gives_one_quad_per_line_and_snaps_reversed_drags_to_the_same_range() {
    let content = "BT /F1 10 Tf 1 0 0 1 20 300 Tm (one two) Tj 1 0 0 1 20 288 Tm (three) Tj 1 0 0 1 20 276 Tm (four) Tj ET";
    let layout = build(&[content], &Font::default(), 0)
        .text_layout(0, false)
        .unwrap();
    // From inside "two" (line 1) to inside "four" (line 3), drawn forward and backward.
    let (a, b) = (P(40.0, dy(300.0) - 3.0), P(27.0, dy(276.0) - 3.0));
    let range = layout.select(a, b).unwrap();
    assert_eq!(layout.select(b, a).unwrap(), range);
    assert_eq!(range.text, "two three fo");
    assert_eq!(range.quads.len(), 3, "one quad per line");
    // Each quad is as tall as one glyph box (10 pt), never a union of the lines.
    for q in &range.quads {
        assert!(close(q.0[3].y - q.0[0].y, 10.0), "{q:?}");
    }
    // Line 1 covers "two" only (x 40..55), line 3 "fo" (x 20..30).
    assert!(close(range.quads[0].0[0].x, 40.0) && close(range.quads[0].0[1].x, 55.0));
    assert!(close(range.quads[2].0[0].x, 20.0) && close(range.quads[2].0[1].x, 30.0));
    assert!(
        layout.select(P(0.0, 0.0), P(1.0, 1.0)).is_some(),
        "ends snap to the nearest cluster"
    );
}

#[test]
fn a_range_cutting_a_cluster_reports_partial_with_the_whole_cluster_quad() {
    let font = Font {
        to_unicode: vec![(1, "fi")],
        unnamed: false,
    };
    let layout = build(
        &["BT /F1 10 Tf 1 0 0 1 20 300 Tm (a\\001b) Tj ET"],
        &font,
        0,
    )
    .text_layout(0, false)
    .unwrap();
    // "a" 0..1, the fi glyph 1..3, "b" 3..4.
    let cut = layout.quads_for(2, 4).unwrap();
    assert!(cut.partial_cluster);
    assert_eq!(cut.text, "ib");
    assert!(
        close(cut.quads[0].0[0].x, 25.0),
        "the whole fi glyph is in the quad: {:?}",
        cut.quads
    );
    assert!(close(width(&cut.quads[0]), 10.0));
    let whole = layout.quads_for(1, 3).unwrap();
    assert!(!whole.partial_cluster && close(width(&whole.quads[0]), 5.0));
    assert!(layout.quads_for(3, 99).is_err() && layout.quads_for(3, 2).is_err());
}

#[test]
fn columns_do_not_merge_into_giant_quads() {
    // Two columns, two lines each. Text order is (y1, x0), so a selection from the left
    // column's line 1 to its line 2 also spans the right column's line 1, but every quad stays
    // inside one column line.
    let content = "BT /F1 10 Tf 1 0 0 1 20 300 Tm (left1) Tj 1 0 0 1 20 288 Tm (left2) Tj \\
                   1 0 0 1 200 300 Tm (right1) Tj 1 0 0 1 200 288 Tm (right2) Tj ET";
    let layout = build(&[content], &Font::default(), 0)
        .text_layout(0, false)
        .unwrap();
    assert_eq!(layout.page.text, "left1 right1 left2 right2");
    let range = layout
        .select(P(22.0, dy(300.0) - 3.0), P(22.0, dy(288.0) - 3.0))
        .unwrap();
    assert_eq!(range.quads.len(), 3);
    assert!(
        range.quads.iter().all(|q| width(q) < 31.0),
        "{:?}",
        range.quads
    );
}

#[test]
fn rotated_text_gets_oriented_union_quads_and_hits() {
    // Text rotated 30 degrees counterclockwise (PDF): baseline direction (cos, sin) in user
    // space, which is (cos, -sin) in display space.
    let (c, s) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
    let content = format!("BT /F1 10 Tf {c} {s} {} {c} 50 100 Tm (abcd) Tj ET", -s);
    let layout = build(&[&content], &Font::default(), 0)
        .text_layout(0, false)
        .unwrap();
    let all = layout.quads_for(0, 4).unwrap();
    assert_eq!(all.quads.len(), 1);
    let q = all.quads[0];
    // Edge TL->TR runs along the baseline at -30 degrees in display space and is 4 glyphs long.
    let (ex, ey) = (q.0[1].x - q.0[0].x, q.0[1].y - q.0[0].y);
    assert!(close(ex.hypot(ey), 20.0));
    assert!((ey.atan2(ex).to_degrees() + 30.0).abs() < 1e-6, "{q:?}");
    // A point on the second glyph's centre hits it even though its box is rotated.
    let centre = P(50.0 + 7.5 * c - 4.0 * s, dy(100.0) - (7.5 * s + 4.0 * c));
    assert_eq!(layout.hit_test(centre, 0.0).unwrap().start, 1);
}
