//! Precise region subtraction (ADR 0009, spec §5.4 rule 4), the shared helper of the vector
//! stage. Everything runs in display space, where regions are axis-aligned rectangles.
//!
//! One group of subpaths (subpaths whose bounds touch, so fill rules cannot reach beyond it) is
//! flattened to [`FLATNESS_PT`], tested against the regions inflated by [`GUARD_PT`], and, only if
//! its ink really meets one, replaced by `ink minus regions` from `i_overlay` on a 1/1024 pt grid.
//! Hard caps are checked before any `i_overlay` call: its infallible entry points panic on bad
//! input and the release profile aborts on panic.
//!
//! A stroke (NEW-87) is first expanded to its outline by kurbo, in the path's own space with its own
//! width, caps, joins, miter limit and dashes (exactly what the renderer draws, so a non-uniform
//! CTM stays exact), mapped to display space and cut like a non-zero fill.
use super::plan::covered;
use crate::interpret::{StrokeProps, widest_axis};
use crate::limits::Budget;
use foliark_model::PdfError;
use hayro::vello_cpu::peniko::kurbo::{
    Affine, BezPath, Cap, ParamCurveArclen, PathEl, Point, Shape, Stroke, StrokeOpts, flatten,
    stroke,
};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::scale::FixedScaleFloatOverlay;
use i_overlay::float::simplify::SimplifyShape;
use std::collections::HashMap;

/// Curve flattening tolerance, display space.
pub(super) const FLATNESS_PT: f64 = 0.01;
/// Regions grow by this before subtraction: grid rounding then never leaves a vertex inside.
pub(super) const GUARD_PT: f64 = 0.01;
/// Fixed-point grid of the boolean, units per point (1/1024 pt).
pub(super) const SCALE: f64 = 1024.0;
/// Largest display coordinate of a group that reaches a region.
pub(super) const MAX_COORD_PT: f64 = 131_072.0;
/// Flattened vertices of one group.
pub(super) const MAX_VERTICES: usize = 250_000;
/// Dash segments of one stroked subpath, estimated before kurbo runs.
pub(super) const MAX_DASHES: usize = 100_000;

pub(super) type Contour = Vec<[f64; 2]>;
type Box4 = [f64; 4];

/// The regions of a page, each grown by [`GUARD_PT`].
pub(super) struct Regions(Vec<Box4>);

impl Regions {
    pub(super) fn new(boxes: &[Box4]) -> Result<Self, PdfError> {
        let ok = |b: &Box4| b.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD_PT);
        if !boxes.iter().all(ok) {
            return Err(PdfError::limit("vectorCoordinate", 0, MAX_COORD_PT as u64));
        }
        Ok(Self(
            boxes
                .iter()
                .map(|b| {
                    [
                        b[0] - GUARD_PT,
                        b[1] - GUARD_PT,
                        b[2] + GUARD_PT,
                        b[3] + GUARD_PT,
                    ]
                })
                .collect(),
        ))
    }

    fn near(&self, b: Box4) -> Vec<Box4> {
        let hit = |r: &&Box4| r[0] < b[2] && b[0] < r[2] && r[1] < b[3] && b[1] < r[3];
        self.0.iter().filter(hit).copied().collect()
    }
}

#[derive(Debug, Clone)]
pub(super) enum Cut {
    /// The ink stays clear of every region: keep the original operators byte for byte.
    Untouched,
    /// All of the ink lies inside the regions.
    Removed,
    /// What remains: disjoint contours (holes wind against their outer), valid under either
    /// fill rule.
    Pieces(Vec<Contour>),
}

fn too_big(limit: &str, actual: usize, max: usize) -> PdfError {
    PdfError::limit(limit, actual as u64, max as u64)
}

/// `group` (display space) flattened to closed polygons; at most [`MAX_VERTICES`] vertices.
fn flat(group: &BezPath) -> Result<Vec<Contour>, PdfError> {
    let (mut out, mut n): (Vec<Contour>, usize) = (Vec::new(), 0);
    let mut over = false;
    flatten(group.elements().iter().copied(), FLATNESS_PT, |el| {
        let p = match el {
            PathEl::MoveTo(p) => {
                out.push(Vec::new());
                p
            }
            PathEl::LineTo(p) => p,
            _ => return,
        };
        n += 1;
        over |= n > MAX_VERTICES;
        if !over && let Some(c) = out.last_mut() {
            c.push([p.x, p.y]);
        }
    });
    if over {
        return Err(too_big("vectorVertices", n, MAX_VERTICES));
    }
    out.retain(|c| c.len() >= 3);
    Ok(out)
}

/// Winding number of `p` in the closed polygons (crossing rule).
fn winding(polys: &[Contour], p: [f64; 2]) -> i32 {
    let mut w = 0;
    for c in polys {
        for (i, a) in c.iter().enumerate() {
            let b = c[(i + 1) % c.len()];
            let cross = (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1]);
            if a[1] <= p[1] && b[1] > p[1] && cross > 0.0 {
                w += 1;
            } else if a[1] > p[1] && b[1] <= p[1] && cross < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

/// Does the closed segment `a`-`b` meet box `r` (Liang-Barsky)?
fn seg_meets(a: [f64; 2], b: [f64; 2], r: Box4) -> bool {
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    for (p, q) in [
        (-dx, a[0] - r[0]),
        (dx, r[2] - a[0]),
        (-dy, a[1] - r[1]),
        (dy, r[3] - a[1]),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

/// Whether the ink (polygons under the fill rule) has any point in a region: an edge meets the
/// box, or no edge does and the box lies inside the ink.
fn touches(
    polys: &[Contour],
    even_odd: bool,
    outline_only: bool,
    near: &[Box4],
    budget: &mut Budget,
) -> Result<bool, PdfError> {
    let edges: usize = polys.iter().map(Vec::len).sum();
    budget.charge((edges as u64).saturating_mul(near.len() as u64 * 2))?;
    for &r in near {
        for c in polys {
            if c.iter()
                .enumerate()
                .any(|(i, a)| seg_meets(*a, c[(i + 1) % c.len()], r))
            {
                return Ok(true);
            }
        }
        // A clip that merely encloses a region holds nothing of it: only its outline is geometry.
        let w = winding(polys, [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]);
        if !outline_only && if even_odd { w % 2 != 0 } else { w != 0 } {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `group` (display space) minus the regions, under its own fill rule. `Untouched` when its ink
/// never reaches a region (for a clip, `outline_only`: when its outline never does), so the
/// caller leaves the stream alone.
pub(super) fn cut(
    group: &BezPath,
    even_odd: bool,
    outline_only: bool,
    regions: &Regions,
    budget: &mut Budget,
) -> Result<Cut, PdfError> {
    let r = group.bounding_box();
    let b = [r.x0, r.y0, r.x1, r.y1];
    if !b.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD_PT) {
        return Err(too_big("vectorCoordinate", 0, MAX_COORD_PT as usize));
    }
    let near = regions.near(b);
    if near.is_empty() {
        return Ok(Cut::Untouched);
    }
    if covered(b, &regions.0) {
        return Ok(Cut::Removed);
    }
    budget.charge(group.elements().len() as u64)?;
    let polys = flat(group)?;
    if polys.is_empty() {
        // A point or a line: zero area, near a region, nothing worth keeping (spec §5.4).
        return Ok(Cut::Removed);
    }
    if !touches(&polys, even_odd, outline_only, &near, budget)? {
        return Ok(Cut::Untouched);
    }
    let n: usize = polys.iter().map(Vec::len).sum::<usize>() + 4 * near.len();
    budget.charge(n as u64 * (u64::from(n.ilog2()) + 1))?;
    let subject: Vec<Contour> = if even_odd {
        polys
            .simplify_shape(FillRule::EvenOdd)
            .into_iter()
            .flatten()
            .collect()
    } else {
        polys
    };
    let clip: Vec<Contour> = near
        .iter()
        .map(|r| vec![[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]])
        .collect();
    let out = subject
        .overlay_with_fixed_scale(&clip, OverlayRule::Difference, FillRule::NonZero, SCALE)
        .map_err(|_| too_big("vectorCoordinate", 0, MAX_COORD_PT as usize))?;
    let pieces: Vec<Contour> = out.into_iter().flatten().collect();
    Ok(if pieces.is_empty() {
        Cut::Removed
    } else {
        Cut::Pieces(pieces)
    })
}

/// A stroked subpath as kurbo draws it: `user` (user space) under the CTM `ctm`, mapped to display
/// space by `u2d`. `Ok(None)` when the ink cannot reach any of `boxes` (a conservative reach,
/// no expansion done); otherwise its display-space outline, to be filled non-zero.
fn stroke_ink(
    user: &BezPath,
    ctm: Affine,
    u2d: Affine,
    props: &StrokeProps,
    boxes: &[Box4],
    budget: &mut Budget,
) -> Result<Option<BezPath>, PdfError> {
    let to_display = u2d * ctm;
    let scale = widest_axis(to_display);
    let r = (u2d * user.clone()).bounding_box();
    let b = [r.x0, r.y0, r.x1, r.y1];
    if !b.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD_PT) || !scale.is_finite() {
        return Err(too_big("vectorCoordinate", 0, MAX_COORD_PT as usize));
    }
    // Half the width times the miter limit or the square-cap corner, along the widest axis.
    let cap = if matches!(props.line_cap, Cap::Square) {
        std::f64::consts::SQRT_2
    } else {
        1.0
    };
    let width = f64::from(props.line_width).max(0.01 / scale.max(1e-9));
    let g = width * scale * 0.5 * f64::from(props.miter_limit).max(1.0).max(cap);
    let hit = |r: &&Box4| r[0] < b[2] + g && b[0] - g < r[2] && r[1] < b[3] + g && b[1] - g < r[3];
    if !boxes.iter().any(|r| hit(&r)) {
        return Ok(None);
    }
    let det = ctm.determinant().abs();
    if scale <= 1e-9 || det <= 1e-12 * scale * scale {
        // A singular CTM collapses the ink to a point or a segment: no area, nothing to keep
        // near a region (the hairline the renderer widens is not worth a cut).
        return Ok(Some(BezPath::new()));
    }
    let dashes = if props.dash_array.iter().all(|d| d.is_finite() && *d >= 0.0)
        && props.dash_array.iter().sum::<f32>() > 1e-6
        && props.dash_offset.is_finite()
    {
        props.dash_array.iter().map(|d| f64::from(*d)).collect()
    } else {
        // An unusable pattern: the solid stroke covers every dash, so nothing is kept wrongly.
        Vec::new()
    };
    let local = ctm.inverse() * user.clone();
    if !dashes.is_empty() {
        let period: f64 =
            dashes.iter().sum::<f64>() * if dashes.len() % 2 == 1 { 2.0 } else { 1.0 };
        let len: f64 = local.segments().map(|s| s.arclen(0.1)).sum();
        let est = len / period * dashes.len() as f64;
        if !est.is_finite() || est > MAX_DASHES as f64 {
            return Err(too_big("vectorDashes", est as usize, MAX_DASHES));
        }
    }
    budget.charge(local.elements().len() as u64)?;
    let style = Stroke {
        width,
        join: props.line_join,
        miter_limit: f64::from(props.miter_limit),
        start_cap: props.line_cap,
        end_cap: props.line_cap,
        dash_pattern: dashes.into_iter().collect(),
        dash_offset: f64::from(props.dash_offset),
    };
    let ink = stroke(
        local.elements().iter().copied(),
        &style,
        &StrokeOpts::default(),
        FLATNESS_PT / scale,
    );
    budget.charge(ink.elements().len() as u64)?;
    Ok(Some(to_display * ink))
}

/// The stroke of one subpath minus the regions (non-zero ink). `Pieces` are stroke OUTLINES,
/// to be filled in the stroking colour.
pub(super) fn cut_stroke(
    user: &BezPath,
    ctm: Affine,
    u2d: Affine,
    props: &StrokeProps,
    regions: &Regions,
    budget: &mut Budget,
) -> Result<Cut, PdfError> {
    match stroke_ink(user, ctm, u2d, props, &regions.0, budget)? {
        None => Ok(Cut::Untouched),
        // Zero-area ink that came within reach of a region: removed, as a fill of no area is.
        Some(ink) if ink.elements().is_empty() => Ok(Cut::Removed),
        Some(ink) => cut(&ink, false, false, regions, budget),
    }
}

/// [`reaches`] for the stroke of one subpath: the written page's own stroke, no guard.
pub(super) fn stroke_reaches(
    user: &BezPath,
    ctm: Affine,
    u2d: Affine,
    props: &StrokeProps,
    boxes: &[Box4],
    budget: &mut Budget,
) -> Result<bool, PdfError> {
    match stroke_ink(user, ctm, u2d, props, boxes, budget)? {
        None => Ok(false),
        Some(ink) if ink.elements().is_empty() => Ok(false),
        Some(ink) => reaches(&ink, false, false, boxes, budget),
    }
}

/// Whether `group` (display space) still has ink (for a clip: outline) in any of `boxes`, as
/// they are, with no guard: the check of spec §5.7 on a written page, independent of the cut.
pub(super) fn reaches(
    group: &BezPath,
    even_odd: bool,
    outline_only: bool,
    boxes: &[Box4],
    budget: &mut Budget,
) -> Result<bool, PdfError> {
    let r = group.bounding_box();
    let b = [r.x0, r.y0, r.x1, r.y1];
    if !b.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD_PT) {
        return Err(too_big("vectorCoordinate", 0, MAX_COORD_PT as usize));
    }
    let hit = |r: &&Box4| r[0] < b[2] && b[0] < r[2] && r[1] < b[3] && b[1] < r[3];
    let near: Vec<Box4> = boxes.iter().filter(hit).copied().collect();
    if near.is_empty() {
        return Ok(false);
    }
    budget.charge(group.elements().len() as u64)?;
    touches(&flat(group)?, even_odd, outline_only, &near, budget)
}

/// The affine taking stream-space points to their display-space images, fitted through the best
/// conditioned triple and verified on every pair. `None` when the points are collinear in the
/// stream or the pairs do not agree on one affine.
pub(super) fn fit(pairs: &[([f64; 2], [f64; 2])]) -> Option<Affine> {
    let (s0, d0) = *pairs.first()?;
    let dist = |p: [f64; 2], q: [f64; 2]| (p[0] - q[0]).hypot(p[1] - q[1]);
    let far = pairs
        .iter()
        .max_by(|a, b| dist(a.0, s0).total_cmp(&dist(b.0, s0)))?;
    let u = [far.0[0] - s0[0], far.0[1] - s0[1]];
    let cross = |p: [f64; 2]| u[0] * (p[1] - s0[1]) - u[1] * (p[0] - s0[0]);
    let third = pairs
        .iter()
        .max_by(|a, b| cross(a.0).abs().total_cmp(&cross(b.0).abs()))?;
    let det = cross(third.0);
    let reach = dist(far.0, s0);
    if !det.is_finite() || det.abs() <= 1e-9 * reach * reach {
        return None;
    }
    let v = [third.0[0] - s0[0], third.0[1] - s0[1]];
    let (du, dv) = (
        [far.1[0] - d0[0], far.1[1] - d0[1]],
        [third.1[0] - d0[0], third.1[1] - d0[1]],
    );
    // [du dv] = M [u v]
    let (a, c) = (
        (du[0] * v[1] - dv[0] * u[1]) / det,
        (dv[0] * u[0] - du[0] * v[0]) / det,
    );
    let (b, d) = (
        (du[1] * v[1] - dv[1] * u[1]) / det,
        (dv[1] * u[0] - du[1] * v[0]) / det,
    );
    let m = Affine::new([
        a,
        b,
        c,
        d,
        d0[0] - a * s0[0] - c * s0[1],
        d0[1] - b * s0[0] - d * s0[1],
    ]);
    let scale = pairs
        .iter()
        .map(|p| p.1[0].abs().max(p.1[1].abs()))
        .fold(1.0, f64::max);
    pairs
        .iter()
        .all(|(s, d)| {
            let q = m * Point::new(s[0], s[1]);
            (q.x - d[0]).abs().max((q.y - d[1]).abs()) <= 1e-6 * scale
        })
        .then_some(m)
}

/// Connected components of `boxes` under overlap (touching counts), as a group id per box in
/// order of first appearance. Subpaths of different groups cannot share a point, so a fill
/// rule never reaches across groups. Sweep over x; every comparison is charged to the budget.
pub(super) fn components(boxes: &[Box4], budget: &mut Budget) -> Result<Vec<usize>, PdfError> {
    fn find(up: &mut [usize], mut i: usize) -> usize {
        while up[i] != i {
            up[i] = up[up[i]];
            i = up[i];
        }
        i
    }
    let mut up: Vec<usize> = (0..boxes.len()).collect();
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by(|&a, &b| boxes[a][0].total_cmp(&boxes[b][0]));
    let mut active: Vec<usize> = Vec::new();
    for i in order {
        active.retain(|&j| boxes[j][2] >= boxes[i][0]);
        budget.charge(active.len() as u64 + 1)?;
        for &j in &active {
            if boxes[j][1] <= boxes[i][3] && boxes[i][1] <= boxes[j][3] {
                let (a, b) = (find(&mut up, i), find(&mut up, j));
                up[a] = b;
            }
        }
        active.push(i);
    }
    let mut ids = HashMap::new();
    Ok((0..boxes.len())
        .map(|i| {
            let root = find(&mut up, i);
            let next = ids.len();
            *ids.entry(root).or_insert(next)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use foliark_model::Limits;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as f64 / (1u64 << 31) as f64
        }
    }

    fn near_edge(polys: &[Contour], p: [f64; 2], band: f64) -> bool {
        polys.iter().any(|c| {
            c.iter().enumerate().any(|(i, a)| {
                let b = c[(i + 1) % c.len()];
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let len2 = dx * dx + dy * dy;
                let t = if len2 == 0.0 {
                    0.0
                } else {
                    (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
                };
                (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy) < band
            })
        })
    }

    /// Random simple and self-intersecting polygons against random region sets: a sample point is
    /// in the cut iff it was inked by the rule and lies outside every grown region (points within
    /// 0.012 pt of any edge are skipped: the grid and the guard move edges by that much).
    #[test]
    fn random_cuts_agree_with_the_pointwise_difference() {
        let mut rng = Lcg(7);
        let mut budget = Budget::new(&Limits::default());
        let (mut pieces, mut removed, mut untouched) = (0, 0, 0);
        for case in 0..300 {
            let n = 3 + (rng.next() * 6.0) as usize;
            let poly: Contour = (0..n)
                .map(|_| [rng.next() * 100.0, rng.next() * 100.0])
                .collect();
            let mut path = BezPath::new();
            path.move_to(Point::new(poly[0][0], poly[0][1]));
            for p in &poly[1..] {
                path.line_to(Point::new(p[0], p[1]));
            }
            path.close_path();
            let even = case % 2 == 0;
            let boxes: Vec<Box4> = (0..1 + (rng.next() * 3.0) as usize)
                .map(|_| {
                    let (x, y) = (rng.next() * 80.0, rng.next() * 80.0);
                    [
                        x,
                        y,
                        x + 5.0 + rng.next() * 35.0,
                        y + 5.0 + rng.next() * 35.0,
                    ]
                })
                .collect();
            let regions = Regions::new(&boxes).unwrap();
            let out = cut(&path, even, false, &regions, &mut budget).unwrap();
            let grown: Vec<Contour> = regions
                .0
                .iter()
                .map(|r| vec![[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]])
                .collect();
            match &out {
                Cut::Untouched => untouched += 1,
                Cut::Removed => removed += 1,
                Cut::Pieces(_) => pieces += 1,
            }
            for _ in 0..400 {
                let p = [rng.next() * 100.0, rng.next() * 100.0];
                if near_edge(std::slice::from_ref(&poly), p, 0.012) || near_edge(&grown, p, 0.012) {
                    continue;
                }
                let w = winding(std::slice::from_ref(&poly), p);
                let inked = if even { w % 2 != 0 } else { w != 0 };
                let in_region = regions
                    .0
                    .iter()
                    .any(|r| r[0] < p[0] && p[0] < r[2] && r[1] < p[1] && p[1] < r[3]);
                let observed = match &out {
                    Cut::Untouched => inked,
                    Cut::Removed => false,
                    Cut::Pieces(c) => winding(c, p) != 0,
                };
                assert_eq!(
                    observed,
                    inked && !in_region,
                    "case {case} point {p:?} even-odd {even}"
                );
            }
        }
        assert!(
            pieces > 50 && untouched > 0,
            "{pieces} {removed} {untouched}"
        );
    }

    #[test]
    fn ink_inside_the_regions_is_removed_and_a_clip_outline_ignores_a_region_it_encloses() {
        let mut budget = Budget::new(&Limits::default());
        let mut tri = BezPath::new();
        tri.move_to(Point::new(10.0, 10.0));
        tri.line_to(Point::new(30.0, 10.0));
        tri.line_to(Point::new(20.0, 30.0));
        tri.close_path();
        let inside = Regions::new(&[[0.0, 0.0, 12.0, 40.0], [11.0, 0.0, 40.0, 40.0]]).unwrap();
        assert!(matches!(
            cut(&tri, false, false, &inside, &mut budget).unwrap(),
            Cut::Removed
        ));
        // A region wholly inside the triangle: the ink has a hole, the outline is untouched.
        let hole = Regions::new(&[[18.0, 14.0, 22.0, 18.0]]).unwrap();
        assert!(matches!(
            cut(&tri, false, false, &hole, &mut budget).unwrap(),
            Cut::Pieces(_)
        ));
        assert!(matches!(
            cut(&tri, false, true, &hole, &mut budget).unwrap(),
            Cut::Untouched
        ));
    }
}
