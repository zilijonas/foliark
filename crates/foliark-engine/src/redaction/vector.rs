//! Vector removal (spec §5.4), executed through the writer. Fills and strokes share one pass:
//! the planner already classified every painted subpath against its display-space bounds
//! (expanded by half the stroke width times the miter limit when stroked), so this module only
//! carries the classes out on the content stream.
//!
//! - (a) `Inside` and (c) `Collateral`: the subpath's construction operators are dropped.
//! - (b) `RectCut` (fill only, an axis-aligned display rectangle on its own): replaced by the
//!   rectangle difference, at most four rectangles per region, written back in the stream's
//!   own space through the exact affine that maps the rectangle's corners to display space.
//! - A painted path that loses everything keeps its `W`/`W*` clip, verbatim, and paints `n`;
//!   without a clip it disappears. Clip-only paths, `sh` and everything not planned are not
//!   touched. Closing paint operators (`s`, `b`, `b*`) are rewritten as their open forms with an
//!   explicit `h` on the last kept subpath, so dropping one subpath never moves a closure.
//!
//! Limits, all typed or reported, none silent:
//! - Exact curve and stroke subtraction is spec §16 "Later": a crossing curve, hole, stroke or
//!   rotated rectangle goes whole and its bounds are reported as collateral.
//! - A singular CTM flattens a path to a line or a point: it is never a rectangle, so it is
//!   removed (`Inside`/`Collateral`) like any other crossing path, and a stroke's reach is
//!   measured along the CTM's widest axis.
//! - A subpath list the stream parser and the model disagree about (a `l` after `h` without
//!   `m`, say) is `INTERNAL`; operators other than path construction inside a path are
//!   `MALFORMED_PDF`. Both are raised before the writer is touched.
//! - Tiling-pattern paint under a region whose cell holds text or images (or whose cell cannot
//!   be resolved) is `REDACT_PATTERN_UNSUPPORTED`: the cell is not interpreted.
//! - A path drawn through a Form or stream that something else draws too is rewritten in a
//!   clone of its occurrence (`forms`); the other occurrences keep the original.
//! - Every operator scanned is charged to the work budget (`maxWorkUnits`).
use super::forms::Targets;
use super::plan::{PlannedVector, RedactionPlan, VectorClass, subpaths};
use super::text::{at, decode};
use crate::Document;
use crate::interpret::{DiagnosticKind, ItemKind, PathItem, SemanticPage};
use crate::limits::{self, Budget};
use crate::reader::object_id;
use crate::writer::Writer;
use foliark_model::{
    Collateral, ErrorCode, ErrorDetails, InstancePath, ObjectId, PdfError, Rect, SourceLoc,
};
use hayro::hayro_syntax::content::UntypedIter;
use hayro::hayro_syntax::object::{Dict, Name, Object, Stream};
use hayro::vello_cpu::peniko::kurbo::{Affine, PathEl, Shape};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::ops::Range;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VectorReport {
    /// Subpaths dropped whole: (a) inside plus (c) collateral.
    pub subpaths_removed: u32,
    /// Rectangles replaced by their difference.
    pub rects_cut: u32,
    /// Subpaths removed although they only crossed a region (§5.4c), with their bounds.
    pub collateral: Vec<Collateral>,
    pub operators_rewritten: u32,
}

type Pt = (f64, f64);

/// One subpath as the content stream wrote it.
#[derive(Default)]
struct Sub {
    ops: Vec<Range<usize>>,
    /// Vertices of `m`, `l` and `re` (curve end points are not needed).
    pts: Vec<Pt>,
    curved: bool,
    /// Closed by `s`, `b` or `b*` rather than by an `h` of its own.
    implicit_h: bool,
}

/// One painting operator with the path it paints.
#[derive(Default)]
struct Painted {
    /// Start of the first path-construction operator.
    first: usize,
    /// End of the `W` / `W*` operator, if the path also clips.
    clip_end: Option<usize>,
    end: usize,
    /// The paint operator with its closing folded away: `S`, `B`, `B*`, `f`, `f*`.
    op: &'static [u8],
    subs: Vec<Sub>,
    /// Tiling pattern names in force for fill and stroke.
    patterns: [Option<String>; 2],
    stray: bool,
}

fn numbers(o: &[&Object<'_>]) -> Vec<f64> {
    o.iter()
        .filter_map(|o| match o {
            Object::Number(n) => Some(n.as_f64()),
            _ => None,
        })
        .collect()
}

/// Every painting operator of `data` whose end offset is in `wanted`.
fn scan(
    data: &[u8],
    wanted: &BTreeSet<u32>,
    budget: &mut Budget,
) -> Result<HashMap<u32, Painted>, PdfError> {
    let mut out = HashMap::new();
    let (mut subs, mut first, mut clip_end, mut stray) = (Vec::<Sub>::new(), None, None, false);
    let mut patterns: [Option<String>; 2] = [None, None];
    let mut saved: Vec<[Option<String>; 2]> = Vec::new();
    let mut it = UntypedIter::new(data);
    while let Some((ins, span)) = it.next_with_span() {
        budget.charge(1)?;
        let operands: Vec<&Object<'_>> = ins.operands().collect();
        let n = numbers(&operands);
        match &**ins.operator {
            op @ (b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h") => {
                first.get_or_insert(span.start);
                if op == b"m" || op == b"re" {
                    let mut sub = Sub::default();
                    match (op, &n[..]) {
                        (b"m", [x, y]) => sub.pts.push((*x, *y)),
                        (b"re", [x, y, w, h]) => {
                            sub.pts
                                .extend([(*x, *y), (x + w, *y), (x + w, y + h), (*x, y + h)])
                        }
                        _ => stray = true,
                    }
                    sub.ops.push(span);
                    subs.push(sub);
                } else if let Some(sub) = subs.last_mut() {
                    sub.ops.push(span);
                    match (op, &n[..]) {
                        (b"l", [x, y]) => sub.pts.push((*x, *y)),
                        (b"c" | b"v" | b"y", _) => sub.curved = true,
                        (b"h", _) => {}
                        _ => stray = true,
                    }
                } else {
                    stray = true;
                }
            }
            b"W" | b"W*" if first.is_some() => clip_end = Some(span.end),
            op @ (b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n") => {
                if let Some(first) = first.filter(|_| wanted.contains(&(span.end as u32))) {
                    if matches!(op, b"s" | b"b" | b"b*")
                        && let Some(last) = subs.last_mut()
                    {
                        last.implicit_h = true;
                    }
                    out.insert(
                        span.end as u32,
                        Painted {
                            first,
                            clip_end,
                            end: span.end,
                            op: match op {
                                b"S" | b"s" => b"S",
                                b"f" | b"F" => b"f",
                                b"f*" => b"f*",
                                b"B" | b"b" => b"B",
                                b"B*" | b"b*" => b"B*",
                                _ => b"n",
                            },
                            subs: std::mem::take(&mut subs),
                            patterns: patterns.clone(),
                            stray,
                        },
                    );
                }
                (subs, first, clip_end, stray) = (Vec::new(), None, None, false);
            }
            b"q" => saved.push(patterns.clone()),
            b"Q" => {
                if let Some(p) = saved.pop() {
                    patterns = p;
                }
            }
            op @ (b"scn" | b"sc" | b"SCN" | b"SC") => {
                let name = match operands.last() {
                    Some(Object::Name(n)) => Some(n.as_str().to_owned()),
                    _ => None,
                };
                patterns[usize::from(op[0] == b'S')] = name;
            }
            // Anything else inside a path would be swallowed by the span replacement.
            _ if first.is_some() => stray = true,
            _ => {}
        }
    }
    Ok(out)
}

// ---- rectangle cuts ---------------------------------------------------------------------------

type Box4 = [f64; 4];

/// `b` without `r` (the part of `b` outside `r`), as at most four disjoint boxes.
fn minus(b: Box4, r: Box4) -> Vec<Box4> {
    let (x0, x1) = (b[0].max(r[0]), b[2].min(r[2]));
    let (y0, y1) = (b[1].max(r[1]), b[3].min(r[3]));
    if x0 >= x1 || y0 >= y1 {
        return vec![b];
    }
    [
        [b[0], b[1], r[0], b[3]],
        [r[2], b[1], b[2], b[3]],
        [x0, b[1], x1, r[1]],
        [x0, r[3], x1, b[3]],
    ]
    .into_iter()
    .filter(|p| p[2] - p[0] > 1e-9 && p[3] - p[1] > 1e-9)
    .collect()
}

/// The affine that takes display space back to the stream's space, from a rectangle's corners
/// in both: `local = p0 + u * a + v * b` with `(a, b)` solving `d - d0 = a * s + b * t`.
struct Back {
    d0: Pt,
    p0: Pt,
    s: Pt,
    t: Pt,
    u: Pt,
    v: Pt,
}

impl Back {
    /// `None` when either quadrilateral is not a parallelogram with area (a singular CTM, a
    /// stream the model reads differently).
    fn new(local: &[Pt], display: &[Pt]) -> Option<Self> {
        let (p, d) = (local.get(..4)?, display.get(..4)?);
        let sub = |a: Pt, b: Pt| (a.0 - b.0, a.1 - b.1);
        let (u, v, s, t) = (
            sub(p[1], p[0]),
            sub(p[3], p[0]),
            sub(d[1], d[0]),
            sub(d[3], d[0]),
        );
        let scale = s.0.abs().max(s.1.abs()).max(t.0.abs()).max(t.1.abs());
        let near = |a: Pt, b: Pt, k: f64| (a.0 - b.0).abs() + (a.1 - b.1).abs() <= 1e-6 * k;
        let local_scale = u.0.abs().max(u.1.abs()).max(v.0.abs()).max(v.1.abs());
        let det = s.0 * t.1 - s.1 * t.0;
        let flat = det.abs() <= 1e-9 * scale * scale || scale == 0.0;
        let det_local = u.0 * v.1 - u.1 * v.0;
        (!flat
            && det_local.abs() > 1e-12 * local_scale * local_scale
            && near(d[2], (d[0].0 + s.0 + t.0, d[0].1 + s.1 + t.1), scale)
            && near(p[2], (p[0].0 + u.0 + v.0, p[0].1 + u.1 + v.1), local_scale))
        .then_some(Self {
            d0: d[0],
            p0: p[0],
            s,
            t,
            u,
            v,
        })
    }

    fn local(&self, x: f64, y: f64) -> Pt {
        let (dx, dy) = (x - self.d0.0, y - self.d0.1);
        let det = self.s.0 * self.t.1 - self.s.1 * self.t.0;
        let a = (self.t.1 * dx - self.t.0 * dy) / det;
        let b = (self.s.0 * dy - self.s.1 * dx) / det;
        (
            self.p0.0 + self.u.0 * a + self.v.0 * b,
            self.p0.1 + self.u.1 * a + self.v.1 * b,
        )
    }
}

/// What happens to one subpath.
enum Fate {
    Drop,
    /// Replacement polygons (four points each) in the stream's space.
    Pieces(Vec<[Pt; 4]>),
}

fn corners(sub: &[PathEl]) -> Vec<Pt> {
    let mut pts: Vec<Pt> = sub
        .iter()
        .filter_map(|e| match e {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => Some((p.x, p.y)),
            _ => None,
        })
        .collect();
    if pts.len() == 5 && pts[0] == pts[4] {
        pts.pop();
    }
    pts
}

fn number(v: f64) -> Result<String, PdfError> {
    if !v.is_finite() {
        return Err(PdfError::new(ErrorCode::Internal));
    }
    // Display never uses an exponent; a -0 reads as 0.
    Ok(format!("{}", v + 0.0))
}

/// The operators that replace the span of `p`.
fn rewrite(data: &[u8], p: &Painted, fates: &HashMap<usize, Fate>) -> Result<Vec<u8>, PdfError> {
    let mut kept = Vec::new();
    for (i, sub) in p.subs.iter().enumerate() {
        match fates.get(&i) {
            None => {
                for r in &sub.ops {
                    kept.extend_from_slice(&data[r.clone()]);
                    kept.push(b'\n');
                }
                if sub.implicit_h {
                    kept.extend_from_slice(b"h\n");
                }
            }
            Some(Fate::Drop) => {}
            Some(Fate::Pieces(pieces)) => {
                for q in pieces {
                    let mut s = String::new();
                    for (n, (x, y)) in q.iter().enumerate() {
                        let _ = writeln!(
                            s,
                            "{} {} {}",
                            number(*x)?,
                            number(*y)?,
                            if n == 0 { "m" } else { "l" }
                        );
                    }
                    s.push_str("h\n");
                    kept.extend_from_slice(s.as_bytes());
                }
            }
        }
    }
    let mut out = Vec::new();
    if let Some(end) = p.clip_end {
        // The clip is not painted: it stays exactly as it was.
        out.extend_from_slice(&data[p.first..end]);
        out.extend_from_slice(b"\nn\n");
    }
    if !kept.is_empty() {
        out.extend_from_slice(&kept);
        out.extend_from_slice(p.op);
        out.push(b'\n');
    }
    Ok(out)
}

// ---- patterns ---------------------------------------------------------------------------------

/// Whether a tiling cell draws text or images (or something this cannot follow).
fn cell_is_unsupported(
    doc: &Document,
    cell: &Stream<'_>,
    depth: u32,
    budget: &mut Budget,
) -> Result<bool, PdfError> {
    if depth > 8 {
        return Ok(true);
    }
    let reader = doc.reader();
    let data = limits::decode_stream(
        cell,
        object_id(cell.obj_id().into()),
        reader.limits(),
        budget,
    )?;
    let xobjects = cell
        .dict()
        .get::<Dict<'_>>("Resources")
        .and_then(|r| r.get::<Dict<'_>>("XObject"));
    let mut it = UntypedIter::new(&data);
    while let Some(ins) = it.next() {
        budget.charge(1)?;
        match &**ins.operator {
            b"BT" | b"BI" | b"Tj" | b"TJ" | b"'" | b"\"" => return Ok(true),
            b"scn" | b"SCN" if matches!(ins.operands().last(), Some(Object::Name(_))) => {
                return Ok(true);
            }
            b"Do" => {
                let Some(Object::Name(n)) = ins.operands().next() else {
                    return Ok(true);
                };
                let Some(x) = xobjects
                    .as_ref()
                    .and_then(|d| d.get::<Stream<'_>>(n.as_str()))
                else {
                    return Ok(true);
                };
                let form = x
                    .dict()
                    .get::<Name<'_>>("Subtype")
                    .is_some_and(|s| s.as_str() == "Form");
                if !form || cell_is_unsupported(doc, &x, depth + 1, budget)? {
                    return Ok(true);
                }
            }
            _ => {}
        }
    }
    Ok(false)
}

// ---- entry point ------------------------------------------------------------------------------

type Index<'m> = HashMap<(SourceLoc, InstancePath), &'m PathItem>;

fn path_items(model: &SemanticPage) -> Index<'_> {
    model
        .items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Path(p) if !p.clip_only => Some(((i.loc, i.path.clone()), p)),
            _ => None,
        })
        .collect()
}

fn box_of(r: &Rect) -> Box4 {
    [r.x, r.y, r.x + r.width, r.y + r.height]
}

/// Remove the vector subpaths of `plan` from `w`'s output. `doc` is the working copy the plan
/// was built from (and `w` writes). Every refusal is raised before `w` is touched; on `Err`
/// drop the writer anyway.
pub fn redact_vectors(
    doc: &Document,
    plan: &RedactionPlan,
    w: &mut Writer<'_>,
) -> Result<VectorReport, PdfError> {
    let mut budget = Budget::new(doc.reader().limits());
    let targets = Targets::new(doc, plan)?;
    let mut wanted: BTreeMap<ObjectId, BTreeSet<u32>> = BTreeMap::new();
    // (page, stream, end, resource owner) of tiling paint under a region.
    let mut tiling: Vec<(u32, ObjectId, u32, ObjectId)> = Vec::new();
    let mut models: HashMap<u32, SemanticPage> = HashMap::new();

    for page in &plan.pages {
        for v in &page.vectors {
            wanted.entry(v.loc.stream).or_default().insert(v.loc.end);
        }
        if page.vectors.is_empty() && page.patterns.is_empty() {
            continue;
        }
        let model = doc.page_model(page.page_index as usize)?;
        if !page.patterns.is_empty() {
            let locs: HashSet<SourceLoc> = model
                .diagnostics
                .iter()
                .filter(|d| d.kind == DiagnosticKind::PatternNotDescended)
                .filter_map(|d| d.loc)
                .collect();
            let to_display = Affine::new(model.geometry.user_to_display.0);
            for item in &model.items {
                let ItemKind::Path(p) = &item.kind else {
                    continue;
                };
                if p.clip_only || !locs.contains(&item.loc) {
                    continue;
                }
                let r = (to_display * p.geometry.clone()).bounding_box();
                let b = [r.x0, r.y0, r.x1, r.y1];
                let under = page.regions.iter().any(|g| {
                    let g = box_of(&g.requested);
                    b[0] <= g[2] && g[0] <= b[2] && b[1] <= g[3] && g[1] <= b[3]
                });
                if under {
                    wanted
                        .entry(item.loc.stream)
                        .or_default()
                        .insert(item.loc.end);
                    tiling.push((
                        page.page_index,
                        item.loc.stream,
                        item.loc.end,
                        item.resource_owner,
                    ));
                }
            }
        }
        models.insert(page.page_index, model);
    }

    let mut streams: HashMap<ObjectId, Vec<u8>> = HashMap::new();
    let mut painted: HashMap<(ObjectId, u32), Painted> = HashMap::new();
    for (stream, ends) in &wanted {
        let data = decode(doc, *stream)?;
        budget.charge_bytes(data.len())?;
        for (end, p) in scan(&data, ends, &mut budget)? {
            if p.stray {
                return Err(PdfError::at(ErrorCode::MalformedPdf, *stream));
            }
            painted.insert((*stream, end), p);
        }
        streams.insert(*stream, data);
    }

    let mut bad_pages = BTreeSet::new();
    for (page, stream, end, owner) in &tiling {
        let patterns = doc
            .reader()
            .pdf()
            .xref()
            .get::<Dict<'_>>(at(*owner))
            .and_then(|d| d.get::<Dict<'_>>("Resources"))
            .and_then(|r| r.get::<Dict<'_>>("Pattern"));
        let names = painted
            .get(&(*stream, *end))
            .map(|p| p.patterns.iter().flatten().collect::<Vec<_>>())
            .unwrap_or_default();
        let unsupported = names.is_empty()
            || names.iter().any(|n| {
                let cell = patterns
                    .as_ref()
                    .and_then(|d| d.get::<Stream<'_>>(n.as_str()));
                // A cell that cannot be read is as unsafe as one that holds text.
                cell.map_or(Ok(true), |c| cell_is_unsupported(doc, &c, 0, &mut budget))
                    .unwrap_or(true)
            });
        if unsupported {
            bad_pages.insert(*page);
        }
    }
    if !bad_pages.is_empty() {
        return Err(PdfError::with(
            ErrorCode::RedactPatternUnsupported,
            ErrorDetails {
                pages: Some(bad_pages.into_iter().collect()),
                ..ErrorDetails::default()
            },
        ));
    }

    let mut report = VectorReport::default();
    let mut rewrites = Vec::new();
    for page in &plan.pages {
        let Some(model) = models.get(&page.page_index) else {
            continue;
        };
        let to_display = Affine::new(model.geometry.user_to_display.0);
        let index = path_items(model);
        let mut by_op: BTreeMap<(ObjectId, u32, u32), Vec<&PlannedVector>> = BTreeMap::new();
        for v in &page.vectors {
            by_op
                .entry((v.loc.stream, v.loc.start, v.loc.end))
                .or_default()
                .push(v);
        }
        for ((stream, _, end), group) in by_op {
            let internal = || PdfError::at(ErrorCode::Internal, stream);
            let p = painted.get(&(stream, end)).ok_or_else(internal)?;
            let item = index
                .get(&(group[0].loc, group[0].path.clone()))
                .ok_or_else(internal)?;
            let shapes = subpaths(&(to_display * item.geometry.clone()));
            if shapes.len() != p.subs.len() {
                return Err(internal());
            }
            let mut fates = HashMap::new();
            let first_path = group[0].path.clone();
            for v in group {
                let i = v.subpath as usize;
                let (sub, shape) = (p.subs.get(i).ok_or_else(internal)?, &shapes[i]);
                let bounds = || Collateral {
                    page_index: page.page_index,
                    bounds: v.bounds,
                };
                let cut = (v.class == VectorClass::RectCut && !sub.curved)
                    .then(|| Back::new(&sub.pts, &corners(shape.elements())))
                    .flatten();
                match (v.class, cut) {
                    (VectorClass::RectCut, Some(back)) => {
                        let d = corners(shape.elements());
                        let xs = d.iter().map(|p| p.0);
                        let ys = d.iter().map(|p| p.1);
                        let rect = [
                            xs.clone().fold(f64::INFINITY, f64::min),
                            ys.clone().fold(f64::INFINITY, f64::min),
                            xs.fold(f64::NEG_INFINITY, f64::max),
                            ys.fold(f64::NEG_INFINITY, f64::max),
                        ];
                        let mut pieces = vec![rect];
                        for &g in &v.regions {
                            let r = box_of(&page.regions[g as usize].requested);
                            budget.charge(pieces.len() as u64)?;
                            pieces = pieces.into_iter().flat_map(|b| minus(b, r)).collect();
                        }
                        let polys = pieces
                            .iter()
                            .map(|b| {
                                [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])]
                                    .map(|(x, y)| back.local(x, y))
                            })
                            .collect();
                        fates.insert(i, Fate::Pieces(polys));
                        report.rects_cut += 1;
                    }
                    (class, _) => {
                        // Inside, collateral, or a rectangle that could not be cut exactly.
                        fates.insert(i, Fate::Drop);
                        report.subpaths_removed += 1;
                        if class != VectorClass::Inside {
                            report.collateral.push(bounds());
                        }
                    }
                }
            }
            let target = targets.stream(page.page_index, &first_path.0, stream);
            rewrites.push((
                target,
                p.first..p.end,
                rewrite(&streams[&stream], p, &fates)?,
            ));
        }
    }
    report.operators_rewritten = rewrites.len() as u32;
    targets.apply(doc, plan, w)?;
    for (stream, span, ops) in rewrites {
        w.replace_span(stream, span, &ops);
    }
    Ok(report)
}
