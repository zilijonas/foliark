//! Vector removal (spec §5.4), executed through the writer. The planner only says which painted
//! paths and clip outlines may reach a region; `geometry` decides what each one becomes.
//!
//! - A filled path is one unit, cut in groups of subpaths whose bounds touch (fill rules cannot
//!   reach beyond a group). A group clear of every region keeps its operators byte for byte;
//!   one wholly inside is dropped; one that crosses is replaced by `ink minus regions` (curves
//!   become 0.01 pt polylines), written back in the stream's own space through the affine that
//!   maps the stream's vertices to display space.
//! - A clip outline is geometry too: `W`/`W*` (painted or `W n`) gets the same cut, so the
//!   original outline never survives as a clip. A clip wholly inside the regions becomes an
//!   empty clip: nothing visible outside the regions was under it. A painted path's clip is
//!   written after its paint, as the operators order it.
//! - A stroke is cut per subpath (ADR 0009, NEW-87): its kurbo outline minus the regions replaces
//!   the subpath as a filled outline (`q <stroking colour as fill> m l ... h f Q`); subpaths whose
//!   ink clears every region keep their operators and are painted by `S`. In `B`/`B*` the fill is
//!   cut as a fill and painted `f`/`f*` first. A cut stroke whose paint a fill cannot reproduce
//!   (colour inherited from outside the stream, a pattern, `CA` != `ca`, alpha under 1 in a `B`)
//!   is `REDACT_VECTOR_UNSUPPORTED {feature, pages}`, raised before the writer is touched.
//!   Closing paint operators (`s`, `b`, `b*`) are rewritten as their open forms with an explicit
//!   `h` on the last kept subpath, so dropping one subpath never moves a closure. Clip-only paths,
//!   `sh` and everything not planned are not touched.
//!
//! Limits, all typed or reported, none silent:
//! - A group over the vertex or coordinate caps is `LIMIT_EXCEEDED` (`vectorVertices`,
//!   `vectorCoordinate`, `vectorDashes`) before any boolean runs; every boolean is charged to the
//!   work budget.
//! - A subpath list the stream parser and the model disagree about (a `l` after `h` without
//!   `m`, say) is `INTERNAL`; operators other than path construction inside a path are
//!   `MALFORMED_PDF`. Both are raised before the writer is touched.
//! - Tiling-pattern paint under a region whose cell holds text or images (or whose cell cannot
//!   be resolved) is `REDACT_PATTERN_UNSUPPORTED`: the cell is not interpreted.
//! - A path drawn through a Form or stream that something else draws too is rewritten in a
//!   clone of its occurrence (`forms`); the other occurrences keep the original.
//! - A clip the interpreter drops as covering its whole bounding box (`Dummy`) has no model
//!   item and stays: it clips nothing, so it holds no geometry.
use super::forms::{Targets, content_ids};
use super::geometry::{self, Cut, Regions};
use super::plan::{PlannedVector, RedactionPlan, VectorClass, VectorKind, subpaths};
use super::text::{at, decode};
use crate::Document;
use crate::interpret::{DiagnosticKind, ItemKind, PathItem, SemanticPage};
use crate::limits::{self, Budget};
use crate::reader::object_id;
use crate::writer::Writer;
use foliark_model::{ErrorCode, ErrorDetails, InstancePath, ObjectId, PdfError, Rect, SourceLoc};
use hayro::hayro_syntax::content::UntypedIter;
use hayro::hayro_syntax::object::{Dict, Name, Object, Stream};
use hayro::vello_cpu::peniko::kurbo::{Affine, BezPath, PathEl, Point, Shape};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::ops::Range;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VectorReport {
    /// Planned fills, strokes and clip outlines whose ink is gone.
    pub subpaths_removed: u32,
    /// Planned fills, strokes and clip outlines replaced by their difference with the regions.
    pub paths_cut: u32,
    /// Of `paths_cut`, those that had curves flattened or a stroke turned into a fill
    /// (`VECTOR_CUT_APPROXIMATED`).
    pub approximated: u32,
    /// Planned entries whose geometry turned out clear of every region (left byte for byte).
    pub untouched: u32,
    pub operators_rewritten: u32,
}

type Pt = (f64, f64);

/// One subpath as the content stream wrote it.
#[derive(Default)]
struct Sub {
    ops: Vec<Range<usize>>,
    /// Vertices of `m`, `l`, `re` and curve end points, in path order.
    pts: Vec<Pt>,
    curved: bool,
    /// Closed by `s`, `b` or `b*` rather than by an `h` of its own.
    implicit_h: bool,
}

/// The stroking colour in force within one stream, as the operators that set the fill colour the
/// same way (`G` to `g`, `CS` to `cs`, ...). A stream that never sets one inherits it.
#[derive(Clone, Default)]
struct Ink {
    cs: Option<Vec<u8>>,
    color: Option<Vec<u8>>,
    /// Set by `G`, `RG` or `K`, which carry their own colour space.
    device: bool,
    /// `SCN` named a pattern.
    pattern: bool,
}

impl Ink {
    fn fill_ops(&self) -> Option<Vec<u8>> {
        match (&self.cs, &self.color) {
            (_, Some(c)) if self.device => Some(c.clone()),
            (Some(cs), c) => Some([cs.as_slice(), c.as_deref().unwrap_or_default()].concat()),
            _ => None,
        }
    }
}

/// The instruction at `span` with its operator in lower case.
fn mirror(data: &[u8], span: &Range<usize>, op: &[u8]) -> Vec<u8> {
    let mut v = data[span.start..span.end - op.len()].to_vec();
    v.extend(op.to_ascii_lowercase());
    v.push(b'\n');
    v
}

/// One painting operator with the path it paints.
#[derive(Default)]
struct Painted {
    /// Start of the first path-construction operator.
    first: usize,
    /// End of the `W` / `W*` operator, if the path also clips.
    clip_end: Option<usize>,
    clip_even_odd: bool,
    end: usize,
    /// The paint operator with its closing folded away: `S`, `B`, `B*`, `f`, `f*`, or `n`.
    op: &'static [u8],
    subs: Vec<Sub>,
    /// Tiling pattern names in force for fill and stroke.
    patterns: [Option<String>; 2],
    ink: Ink,
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

/// Every painting operator of `data` whose end offset is in `wanted`. `opener`: `data` is the first
/// content stream of a page.
fn scan(
    data: &[u8],
    wanted: &BTreeSet<u32>,
    opener: bool,
    budget: &mut Budget,
) -> Result<HashMap<u32, Painted>, PdfError> {
    let mut out = HashMap::new();
    let (mut subs, mut first, mut clip_end, mut stray) = (Vec::<Sub>::new(), None, None, false);
    let mut clip_even_odd = false;
    let mut patterns: [Option<String>; 2] = [None, None];
    // The first content stream of a page starts from the initial state: stroking colour black.
    let mut ink = Ink {
        color: opener.then(|| b"0 g\n".to_vec()),
        device: opener,
        ..Ink::default()
    };
    let mut saved: Vec<([Option<String>; 2], Ink)> = Vec::new();
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
                        (b"c", [_, _, _, _, x, y]) | (b"v" | b"y", [_, _, x, y]) => {
                            sub.curved = true;
                            sub.pts.push((*x, *y));
                        }
                        (b"h", _) => {}
                        _ => stray = true,
                    }
                } else {
                    stray = true;
                }
            }
            op @ (b"W" | b"W*") if first.is_some() => {
                clip_end = Some(span.end);
                clip_even_odd = op == b"W*";
            }
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
                            clip_even_odd,
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
                            ink: ink.clone(),
                            stray,
                        },
                    );
                }
                (subs, first, clip_end, stray) = (Vec::new(), None, None, false);
            }
            b"q" => saved.push((patterns.clone(), ink.clone())),
            b"Q" => {
                if let Some((p, i)) = saved.pop() {
                    (patterns, ink) = (p, i);
                }
            }
            op @ (b"G" | b"RG" | b"K" | b"CS") => {
                let text = mirror(data, &span, op);
                ink = if op == b"CS" {
                    Ink {
                        cs: Some(text),
                        ..Ink::default()
                    }
                } else {
                    Ink {
                        color: Some(text),
                        device: true,
                        ..Ink::default()
                    }
                };
            }
            op @ (b"scn" | b"sc" | b"SCN" | b"SC") => {
                let name = match operands.last() {
                    Some(Object::Name(n)) => Some(n.as_str().to_owned()),
                    _ => None,
                };
                if op[0] == b'S' {
                    ink.color = Some(mirror(data, &span, op));
                    (ink.device, ink.pattern) = (false, name.is_some());
                }
                patterns[usize::from(op[0] == b'S')] = name;
            }
            // Anything else inside a path would be swallowed by the span replacement.
            _ if first.is_some() => stray = true,
            _ => {}
        }
    }
    Ok(out)
}

// ---- cutting ----------------------------------------------------------------------------------

fn number(v: f64) -> Result<String, PdfError> {
    if !v.is_finite() {
        return Err(PdfError::new(ErrorCode::Internal));
    }
    // Display never uses an exponent; a -0 reads as 0.
    Ok(format!("{}", v + 0.0))
}

/// Path vertices in operator order (`m`, `l`, curve end points), as `scan` lists them.
fn vertices(sub: &BezPath) -> Vec<[f64; 2]> {
    sub.elements()
        .iter()
        .filter_map(|e| match e {
            PathEl::MoveTo(p)
            | PathEl::LineTo(p)
            | PathEl::QuadTo(_, p)
            | PathEl::CurveTo(_, _, p) => Some([p.x, p.y]),
            PathEl::ClosePath => None,
        })
        .collect()
}

/// Cut per group, with each subpath's group.
type Cuts = Option<(Vec<usize>, Vec<Cut>)>;

/// What a painted path (or clip outline) becomes: the fill part per group of subpaths, the
/// stroke part per subpath, the clip per group.
#[derive(Default)]
struct Decided {
    ink: Cuts,
    stroke: Cuts,
    clip: Cuts,
}

impl Decided {
    fn changed(cuts: &Cuts) -> bool {
        cuts.iter()
            .any(|(_, c)| c.iter().any(|c| !matches!(c, Cut::Untouched)))
    }

    fn pieces(cuts: &Cuts) -> bool {
        cuts.iter()
            .any(|(_, c)| c.iter().any(|c| matches!(c, Cut::Pieces(_))))
    }
}

/// The subpaths of `display` joined into one path per group of touching bounds, with each
/// subpath's group.
fn grouped(
    display: &[BezPath],
    budget: &mut Budget,
) -> Result<(Vec<usize>, Vec<BezPath>), PdfError> {
    let boxes: Vec<[f64; 4]> = display
        .iter()
        .map(|s| {
            let r = s.bounding_box();
            [r.x0, r.y0, r.x1, r.y1]
        })
        .collect();
    let group_of = geometry::components(&boxes, budget)?;
    let mut paths = vec![BezPath::new(); group_of.iter().max().map_or(0, |m| m + 1)];
    for (sub, &g) in display.iter().zip(&group_of) {
        paths[g].extend(sub.elements().iter().copied());
    }
    Ok((group_of, paths))
}

/// Cut every group path under `even_odd`.
fn cut_groups(
    paths: &[BezPath],
    even_odd: bool,
    outline_only: bool,
    regions: &Regions,
    budget: &mut Budget,
) -> Result<Vec<Cut>, PdfError> {
    paths
        .iter()
        .map(|p| geometry::cut(p, even_odd, outline_only, regions, budget))
        .collect()
}

/// `m l ... h` for every contour, mapped from display space back into the stream's space.
fn contours(cs: &[geometry::Contour], back: Affine) -> Result<String, PdfError> {
    let mut s = String::new();
    for c in cs {
        for (n, p) in c.iter().enumerate() {
            let q = back * Point::new(p[0], p[1]);
            let _ = writeln!(
                s,
                "{} {} {}",
                number(q.x)?,
                number(q.y)?,
                if n == 0 { "m" } else { "l" }
            );
        }
        s.push_str("h\n");
    }
    Ok(s)
}

/// The path operators of `p` after `cuts`: kept subpaths verbatim, cut groups as their pieces
/// (written once, at the group's first subpath). `h` completes an implicit closing only when
/// `closing`.
fn emit(
    data: &[u8],
    p: &Painted,
    group_of: &[usize],
    cuts: &[Cut],
    back: Option<Affine>,
    closing: bool,
) -> Result<Vec<u8>, PdfError> {
    let mut out = Vec::new();
    let mut written = vec![false; cuts.len()];
    for (sub, &g) in p.subs.iter().zip(group_of) {
        match &cuts[g] {
            Cut::Untouched => {
                for r in &sub.ops {
                    out.extend_from_slice(&data[r.clone()]);
                    out.push(b'\n');
                }
                if closing && sub.implicit_h {
                    out.extend_from_slice(b"h\n");
                }
            }
            Cut::Removed => {}
            Cut::Pieces(c) if !std::mem::replace(&mut written[g], true) => {
                let back = back.ok_or_else(|| PdfError::new(ErrorCode::Internal))?;
                out.extend_from_slice(contours(c, back)?.as_bytes());
            }
            Cut::Pieces(_) => {}
        }
    }
    Ok(out)
}

/// The stroke of `p` after `cuts` (one per subpath): subpaths clear of every region keep their
/// operators and are painted by `S`; the rest are filled outlines in the stroking colour,
/// inside `q ... Q` so later content keeps its fill colour.
fn stroke_part(
    data: &[u8],
    p: &Painted,
    cuts: &[Cut],
    back: Option<Affine>,
    paint: Option<&[u8]>,
) -> Result<Vec<u8>, PdfError> {
    let (mut kept, mut pieces) = (Vec::new(), Vec::new());
    for (sub, cut) in p.subs.iter().zip(cuts) {
        match cut {
            Cut::Untouched => {
                for r in &sub.ops {
                    kept.extend_from_slice(&data[r.clone()]);
                    kept.push(b'\n');
                }
                if sub.implicit_h {
                    kept.extend_from_slice(b"h\n");
                }
            }
            Cut::Pieces(c) => pieces.extend(c.iter().cloned()),
            Cut::Removed => {}
        }
    }
    let mut out = Vec::new();
    if !kept.is_empty() {
        out.extend_from_slice(&kept);
        out.extend_from_slice(b"S\n");
    }
    if !pieces.is_empty() {
        let internal = || PdfError::new(ErrorCode::Internal);
        out.extend_from_slice(b"q\n");
        out.extend_from_slice(paint.ok_or_else(internal)?);
        out.extend_from_slice(contours(&pieces, back.ok_or_else(internal)?)?.as_bytes());
        out.extend_from_slice(b"f\nQ\n");
    }
    Ok(out)
}

/// The operators that replace the span of `p`. `back` maps display space into the stream's space
/// for the fill and clip pieces, `stroke_back` for the stroke's, painted with `paint`.
fn rewrite(
    data: &[u8],
    p: &Painted,
    d: &Decided,
    back: Option<Affine>,
    stroke_back: Option<Affine>,
    paint: Option<&[u8]>,
) -> Result<Vec<u8>, PdfError> {
    let mut out = Vec::new();
    // A `B` whose fill or stroke changed is written as its two halves, fill first.
    let split = d.stroke.is_some() && (Decided::changed(&d.ink) || Decided::changed(&d.stroke));
    if let Some((of, ink)) = &d.ink {
        let body = emit(data, p, of, ink, back, true)?;
        if !body.is_empty() {
            out.extend_from_slice(&body);
            out.extend_from_slice(match (split, p.op) {
                (true, b"B*") => b"f*",
                (true, _) => b"f",
                (false, op) => op,
            });
            out.push(b'\n');
        }
    }
    if let Some((_, cuts)) = &d.stroke
        && (split || p.op == b"S")
    {
        out.extend_from_slice(&stroke_part(data, p, cuts, stroke_back, paint)?);
    }
    if let (Some(_), Some((of, clip))) = (p.clip_end, &d.clip) {
        // The clip outline is geometry: cut like the ink. Nothing left clips everything (a
        // degenerate rectangle), which only hides what the regions already take out.
        let mut body = emit(data, p, of, clip, back, false)?;
        if body.is_empty() {
            body.extend_from_slice(b"0 0 0 0 re\n");
        }
        out.extend_from_slice(&body);
        out.extend_from_slice(if p.clip_even_odd {
            b"W*\nn\n"
        } else {
            b"W\nn\n"
        });
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

/// Spec §5.7 on a written page: does a planned filled path or clip outline (`after`, built on the
/// output against the regions shrunk by a slack) still have ink or outline in a region?
/// `exempt` is the redaction's own opaque fill.
pub fn page_leaks(
    doc: &Document,
    page: &super::plan::PagePlan,
    exempt: &dyn Fn(&Rect) -> bool,
) -> Result<bool, PdfError> {
    let model = doc.page_model(page.page_index as usize)?;
    let to_display = Affine::new(model.geometry.user_to_display.0);
    let index = path_items(&model);
    let boxes: Vec<[f64; 4]> = page.regions.iter().map(|g| box_of(&g.requested)).collect();
    let mut budget = Budget::new(doc.reader().limits());
    for v in page.vectors.iter().filter(|v| v.class == VectorClass::Cut) {
        let item = index
            .get(&(v.loc, v.path.clone(), v.kind))
            .ok_or_else(|| PdfError::at(ErrorCode::Internal, v.loc.stream))?;
        if exempt(&v.bounds) {
            continue;
        }
        if let (VectorKind::Stroke, Some(props)) = (v.kind, &item.stroke) {
            for sub in subpaths(&item.geometry) {
                let hit = geometry::stroke_reaches(
                    &sub,
                    item.ctm,
                    to_display,
                    props,
                    &boxes,
                    &mut budget,
                )?;
                if hit {
                    return Ok(true);
                }
            }
            continue;
        }
        let (_, paths) = grouped(
            &subpaths(&(to_display * item.geometry.clone())),
            &mut budget,
        )?;
        let even = item.fill == Some(crate::interpret::FillRule::EvenOdd);
        for p in &paths {
            if geometry::reaches(p, even, item.clip_only, &boxes, &mut budget)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

// ---- entry point ------------------------------------------------------------------------------

/// The model's path items by operator, instance and what they paint (`B` is a fill and a stroke).
type Index<'m> = HashMap<(SourceLoc, InstancePath, VectorKind), &'m PathItem>;

fn path_items(model: &SemanticPage) -> Index<'_> {
    model
        .items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Path(p) => {
                let kind = match (p.clip_only, p.stroke.is_some()) {
                    (true, _) => VectorKind::Clip,
                    (_, true) => VectorKind::Stroke,
                    _ => VectorKind::Fill,
                };
                Some(((i.loc, i.path.clone(), kind), p))
            }
            _ => None,
        })
        .collect()
}

fn box_of(r: &Rect) -> [f64; 4] {
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

    let mut openers: HashSet<ObjectId> = HashSet::new();
    for page in &plan.pages {
        openers.extend(content_ids(doc, page.page_index as usize)?.first().copied());
    }
    let mut streams: HashMap<ObjectId, Vec<u8>> = HashMap::new();
    let mut painted: HashMap<(ObjectId, u32), Painted> = HashMap::new();
    for (stream, ends) in &wanted {
        let data = decode(doc, *stream)?;
        budget.charge_bytes(data.len())?;
        for (end, p) in scan(&data, ends, openers.contains(stream), &mut budget)? {
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
    // Stroke paint a fill cannot reproduce: feature -> pages.
    let mut refused: BTreeMap<&str, BTreeSet<u32>> = BTreeMap::new();
    for page in &plan.pages {
        let Some(model) = models.get(&page.page_index) else {
            continue;
        };
        let to_display = Affine::new(model.geometry.user_to_display.0);
        let index = path_items(model);
        let boxes: Vec<[f64; 4]> = page.regions.iter().map(|g| box_of(&g.requested)).collect();
        let regions = Regions::new(&boxes)?;
        let mut by_op: BTreeMap<(ObjectId, u32, u32), Vec<&PlannedVector>> = BTreeMap::new();
        for v in &page.vectors {
            by_op
                .entry((v.loc.stream, v.loc.start, v.loc.end))
                .or_default()
                .push(v);
        }
        for ((stream, _, end), group) in by_op {
            let internal = || PdfError::at(ErrorCode::Internal, stream);
            let key = |v: &PlannedVector| (v.loc, v.path.clone(), v.kind);
            let item = index.get(&key(group[0])).ok_or_else(internal)?;
            let Some(p) = painted.get(&(stream, end)) else {
                if item.clip_only {
                    report.untouched += group.len() as u32;
                    continue;
                }
                return Err(internal());
            };
            let display = subpaths(&(to_display * item.geometry.clone()));
            if display.len() != p.subs.len() {
                return Err(internal());
            }
            let first_path = group[0].path.clone();
            let paints_fill = matches!(p.op, b"f" | b"f*" | b"B" | b"B*");
            let paints_stroke = matches!(p.op, b"S" | b"B" | b"B*");
            let mut d = Decided::default();
            if paints_fill || p.clip_end.is_some() {
                let (of, paths) = grouped(&display, &mut budget)?;
                if paints_fill {
                    let even = matches!(p.op, b"f*" | b"B*");
                    let cuts = cut_groups(&paths, even, false, &regions, &mut budget)?;
                    d.ink = Some((of.clone(), cuts));
                }
                if p.clip_end.is_some() {
                    let cuts = cut_groups(&paths, p.clip_even_odd, true, &regions, &mut budget)?;
                    d.clip = Some((of, cuts));
                }
            }
            let mut stroke_item = None;
            if paints_stroke {
                let key = (group[0].loc, first_path.clone(), VectorKind::Stroke);
                let s = *index.get(&key).ok_or_else(internal)?;
                let props = s.stroke.as_ref().ok_or_else(internal)?;
                let cuts = subpaths(&s.geometry)
                    .iter()
                    .map(|sub| {
                        geometry::cut_stroke(sub, s.ctm, to_display, props, &regions, &mut budget)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                d.stroke = Some(((0..cuts.len()).collect(), cuts));
                stroke_item = Some(s);
            }
            if let Some(s) = stroke_item {
                let both_changed =
                    paints_fill && (Decided::changed(&d.ink) || Decided::changed(&d.stroke));
                let pieces = Decided::pieces(&d.stroke);
                let feature = if both_changed && (s.stroke_alpha < 1.0 || s.fill_alpha < 1.0) {
                    Some("strokeBothAlpha")
                } else if pieces && s.stroke_alpha != s.fill_alpha {
                    Some("strokeAlphaDiffers")
                } else if pieces && p.ink.pattern {
                    Some("strokePattern")
                } else if pieces && p.ink.fill_ops().is_none() {
                    Some("strokeColorInherited")
                } else {
                    None
                };
                if let Some(f) = feature {
                    refused.entry(f).or_default().insert(page.page_index);
                    continue;
                }
            }
            let curved = |cuts: &Cuts| {
                cuts.iter().any(|(of, c)| {
                    c.iter().enumerate().any(|(g, c)| {
                        matches!(c, Cut::Pieces(_))
                            && p.subs.iter().zip(of).any(|(s, &sg)| sg == g && s.curved)
                    })
                })
            };
            for v in &group {
                let (cuts, approximated) = match v.kind {
                    VectorKind::Clip => (&d.clip, curved(&d.clip)),
                    // A cut stroke is a fill now: its structure is gone whatever its shape.
                    VectorKind::Stroke if paints_stroke => (&d.stroke, true),
                    _ => (&d.ink, curved(&d.ink)),
                };
                if !Decided::changed(cuts) {
                    report.untouched += 1;
                } else if Decided::pieces(cuts) {
                    report.paths_cut += 1;
                    report.approximated += u32::from(approximated);
                } else {
                    report.subpaths_removed += 1;
                }
            }
            if ![&d.ink, &d.stroke, &d.clip]
                .iter()
                .any(|c| Decided::changed(c))
            {
                continue;
            }
            let back = if Decided::pieces(&d.ink) || Decided::pieces(&d.clip) {
                let pairs: Vec<([f64; 2], [f64; 2])> = p
                    .subs
                    .iter()
                    .zip(&display)
                    .flat_map(|(s, sub)| {
                        s.pts
                            .iter()
                            .zip(vertices(sub))
                            .map(|(a, b)| ([a.0, a.1], b))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                let m = geometry::fit(&pairs).ok_or_else(internal)?;
                Some(m.inverse())
            } else {
                None
            };
            // The stroke's own CTM is exact, so a lone segment (no fit) is fine.
            let stroke_back = stroke_item
                .filter(|_| Decided::pieces(&d.stroke))
                .map(|s| (to_display * s.ctm).inverse());
            let paint = p.ink.fill_ops();
            let target = targets.stream(page.page_index, &first_path.0, stream);
            rewrites.push((
                target,
                p.first..p.end,
                rewrite(
                    &streams[&stream],
                    p,
                    &d,
                    back,
                    stroke_back,
                    paint.as_deref(),
                )?,
            ));
        }
    }
    if let Some((feature, pages)) = refused.into_iter().next() {
        return Err(PdfError::with(
            ErrorCode::RedactVectorUnsupported,
            ErrorDetails {
                feature: Some(feature.into()),
                pages: Some(pages.into_iter().collect()),
                ..ErrorDetails::default()
            },
        ));
    }
    report.operators_rewritten = rewrites.len() as u32;
    targets.apply(doc, plan, w)?;
    for (stream, span, ops) in rewrites {
        w.replace_span(stream, span, &ops);
    }
    Ok(report)
}
