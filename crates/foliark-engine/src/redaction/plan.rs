//! Redaction planning (spec §4, §5): from the committed redaction list of one revision, decide
//! what each page occurrence loses and disclose it before anything is written.
//!
//! The plan reads the working copy's semantic model, never the file bytes, and executes
//! nothing: image erasure, vector subtraction and Form cloning are classified here and carried
//! out by later tasks. So planning never needs vector subtraction to succeed. A region applies
//! to its own page occurrence only (spec §5): "all occurrences" is the caller's choice of
//! submitting one redaction per search hit. All geometry is display space (spec §2).
use super::text::at;
use crate::Document;
use crate::document::{Binding, Committed};
use crate::engine::Engine;
use crate::interpret::{DiagnosticKind, GlyphItem, Item, ItemKind, PathItem, SemanticPage};
use crate::text::ClusterId;
use crate::writer::fonts::program_of;
use foliark_model::{
    ErrorCode, InstancePath, InstanceStep, Loss, LossCode, Matrix, ObjectId, PdfError, Point, Quad,
    Rect, RedactionReport, SharedKind, SharedSurvivor, SourceLoc,
};
use hayro::hayro_syntax::object::{Array, Dict, Name};
use hayro::vello_cpu::peniko::kurbo::{Affine, BezPath, Cap, PathEl, Shape};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Spec §5.1: a cluster goes when this share of its quad lies in a region.
const TEXT_OVERLAP: f64 = 0.02;

/// What one revision's redactions do to its working copy. Valid only for the revision in
/// `binding` (check with [`RedactionPlan::ensure_current`] before using it).
#[derive(Debug, Clone)]
pub struct RedactionPlan {
    pub binding: Binding,
    /// Pages that have a region, ascending.
    pub pages: Vec<PagePlan>,
    /// For each font that loses a glyph: every distinct glyph of it that survives anywhere in
    /// the document (rebuilding ToUnicode and emptying TrueType outlines needs the whole
    /// document, not the page), and those of every font drawing from the same embedded program
    /// (Word's WinAnsi TrueType and its Identity-H twin), whose outlines must stay too.
    pub font_survivors: BTreeMap<ObjectId, Vec<GlyphItem>>,
    /// Pages that draw each content stream holding a removed cluster. More than one page
    /// means a shared stream, which an in-place rewrite would change on every page.
    pub stream_pages: BTreeMap<ObjectId, BTreeSet<u32>>,
    /// The counts the export report will carry, as planned.
    pub report: RedactionReport,
    /// Losses export will incur, known before it runs.
    pub losses: Vec<Loss>,
}

#[derive(Debug, Clone)]
pub struct PagePlan {
    pub page_index: u32,
    /// Every region of every committed redaction on the page, with what it reaches.
    pub regions: Vec<PlannedRegion>,
    pub clusters: Vec<PlannedCluster>,
    pub images: Vec<PlannedImage>,
    pub vectors: Vec<PlannedVector>,
    /// Form occurrences that an affected item is drawn through, innermost last per item.
    pub forms: Vec<PlannedForm>,
    /// Annotations whose rectangle meets a region (their appearance is flattening's concern;
    /// the hidden data they carry is dropped by sanitization under the strict policy).
    pub annotations: Vec<PlannedAnnotation>,
    /// Tiling-pattern paint under a region. The cell is not interpreted, so whether it holds
    /// text or images (`REDACT_PATTERN_UNSUPPORTED`, §5.4) is for the vector task to decide.
    pub patterns: Vec<Rect>,
    /// Regions that meet that paint (indices into `regions`).
    pub pattern_regions: Vec<u32>,
    /// Regions that meet content drawn under an ExtGState soft mask or that mask group's BBox
    /// (indices into `regions`). The mask group is not in the model, so nothing executes these.
    pub soft_masks: Vec<u32>,
}

/// One region of a redaction and what it effectively takes out: `effective` is the box of the
/// requested rectangle and of everything removed or erased because of it, so a partly covered
/// glyph, image or vector is disclosed whole.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedRegion {
    pub redaction: String,
    pub requested: Rect,
    pub effective: Rect,
    pub fill: Option<[f64; 3]>,
    pub clusters: u32,
    /// Clusters the region only partly covers (removed whole).
    pub partial_clusters: u32,
    pub images: u32,
    pub vectors: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterReason {
    /// `area(quad ∩ region) >= 2%` of the quad.
    Overlap,
    /// In a text redaction's must-remove set only.
    MustRemove,
    Both,
}

#[derive(Debug, Clone)]
pub struct PlannedCluster {
    pub id: ClusterId,
    pub font: Option<ObjectId>,
    /// The character code as shown (the sanity check of a `TJ` rewrite).
    pub code: u32,
    pub quad: Quad,
    pub reason: ClusterReason,
    /// Largest share of the quad inside one region (1 = fully inside).
    pub coverage: f64,
    /// The glyph's advance in em along its writing direction, Tc and Tw included: what the
    /// removed code must leave behind as a `TJ` displacement of `-1000 * advance_em`.
    pub advance_em: f64,
    /// The mapping has more than one code point (a ligature): it is still one cluster.
    pub multi_codepoint: bool,
    /// Render mode 3 or 7 (OCR layer).
    pub invisible: bool,
    pub regions: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct PlannedImage {
    pub loc: SourceLoc,
    pub path: InstancePath,
    /// `None` for an inline image (it becomes an XObject when erased).
    pub obj: Option<ObjectId>,
    pub quad: Quad,
    /// The XObject is drawn more than once in the document: the erased occurrence needs a clone.
    pub needs_clone: bool,
    pub regions: Vec<u32>,
}

/// Spec §5.4 classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorClass {
    /// (a) inside the union of regions: removed.
    Inside,
    /// (b) a filled path or a clip outline whose bounds reach a region: the geometry stage
    /// decides (untouched, removed, or replaced by its difference with the regions).
    Cut,
}

/// What a planned entry paints: `B` is a fill entry and a stroke entry, a clipping path adds a
/// clip entry; each is cut on its own (ADR 0009).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VectorKind {
    Fill,
    Stroke,
    Clip,
}

#[derive(Debug, Clone)]
pub struct PlannedVector {
    pub loc: SourceLoc,
    pub path: InstancePath,
    pub kind: VectorKind,
    pub class: VectorClass,
    /// Display bounds, expanded by half the stroke width times the miter limit when stroked.
    pub bounds: Rect,
    pub regions: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedForm {
    /// The Form instances from the page down to and including this one.
    pub path: InstancePath,
    pub form: ObjectId,
    /// The Form object is drawn more than once in the document, so it must be cloned rather
    /// than edited in place.
    pub shared: bool,
    /// The `Do` that draws this occurrence, in the stream of its parent (the page's content or
    /// the enclosing Form).
    pub do_loc: SourceLoc,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedAnnotation {
    pub obj: Option<ObjectId>,
    pub subtype: String,
    pub bounds: Rect,
    pub regions: Vec<u32>,
}

impl RedactionPlan {
    /// Plan `committed` against `doc`, the working copy at `binding`. Scans every page: the
    /// shared-resource report needs each occurrence, not only those under a region.
    pub fn build(
        doc: &Document,
        committed: &Committed,
        binding: Binding,
    ) -> Result<Self, PdfError> {
        let pages = doc.page_count();
        let mut by_page: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (i, r) in committed.redactions.iter().enumerate() {
            if r.page_index as usize >= pages {
                return Err(PdfError::page_out_of_range(r.page_index as usize));
            }
            by_page.entry(r.page_index).or_default().push(i);
        }
        let mut scan = Scan::default();
        let mut out = Vec::new();
        if !by_page.is_empty() {
            for p in 0..pages {
                let model = doc.page_model(p)?;
                let page = match by_page.get(&(p as u32)) {
                    Some(list) => {
                        let stored: Vec<_> =
                            list.iter().map(|&i| &committed.redactions[i]).collect();
                        Some(plan_page(doc, &model, &stored, &mut scan)?)
                    }
                    None => {
                        scan.untouched(&model);
                        None
                    }
                };
                out.extend(page);
            }
        }
        let xref = doc.reader().pdf().xref();
        let program: HashMap<ObjectId, ObjectId> = scan
            .survivors
            .keys()
            .chain(&scan.touched_fonts)
            .filter_map(|&f| Some((f, program_of(&xref.get::<Dict<'_>>(at(f))?)?)))
            .collect();
        Ok(scan.finish(binding, out, &program))
    }

    /// `STALE_PLAN` unless the document is still at the plan's revision (spec §4).
    pub fn ensure_current(&self, engine: &Engine) -> Result<(), PdfError> {
        if engine.is_current(&self.binding) {
            Ok(())
        } else {
            Err(PdfError::new(ErrorCode::StalePlan))
        }
    }

    /// The clusters planned for removal on `page`, by id.
    pub fn removed(&self) -> HashSet<&ClusterId> {
        self.pages
            .iter()
            .flat_map(|p| p.clusters.iter().map(|c| &c.id))
            .collect()
    }
}

// ---- geometry ---------------------------------------------------------------------------------

type B = [f64; 4];

pub(crate) fn bounds_of(r: &Rect) -> B {
    [r.x, r.y, r.x + r.width, r.y + r.height]
}

fn rect_of(b: B) -> Rect {
    Rect {
        x: b[0],
        y: b[1],
        width: b[2] - b[0],
        height: b[3] - b[1],
    }
}

fn union(a: B, b: B) -> B {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

fn axis(alo: f64, ahi: f64, blo: f64, bhi: f64) -> bool {
    if alo < ahi && blo < bhi {
        alo < bhi && blo < ahi
    } else {
        // A zero-extent side (a hairline, a zero-width glyph) overlaps by touching.
        alo <= bhi && blo <= ahi
    }
}

pub(crate) fn overlap(a: B, b: B) -> bool {
    axis(a[0], a[2], b[0], b[2]) && axis(a[1], a[3], b[1], b[3])
}

fn quad_bounds(q: &Quad) -> B {
    let r = q.bounds();
    bounds_of(&r)
}

fn area(p: &[Point]) -> f64 {
    let n = p.len();
    (0..n)
        .map(|i| p[i].x * p[(i + 1) % n].y - p[(i + 1) % n].x * p[i].y)
        .sum::<f64>()
        .abs()
        / 2.0
}

/// Area of a convex polygon inside the rectangle `r` (Sutherland-Hodgman).
fn clipped_area(poly: &[Point], r: B) -> f64 {
    let mut pts = poly.to_vec();
    for (vertical, bound, keep_above) in [
        (true, r[0], true),
        (true, r[2], false),
        (false, r[1], true),
        (false, r[3], false),
    ] {
        let v = |p: &Point| if vertical { p.x } else { p.y };
        let inside = |p: &Point| {
            if keep_above {
                v(p) >= bound
            } else {
                v(p) <= bound
            }
        };
        let mut next = Vec::new();
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            if inside(&a) {
                next.push(a);
            }
            if inside(&a) != inside(&b) {
                let t = (bound - v(&a)) / (v(&b) - v(&a));
                next.push(Point {
                    x: a.x + t * (b.x - a.x),
                    y: a.y + t * (b.y - a.y),
                });
            }
        }
        if next.is_empty() {
            return 0.0;
        }
        pts = next;
    }
    area(&pts)
}

/// The share of `q` inside `r` when that meets the §5.1 threshold. A quad without area (a
/// zero-advance glyph) counts, fully, when its box touches the region.
pub(crate) fn text_coverage(q: &Quad, r: B) -> Option<f64> {
    let a = area(&q.0);
    if a < 1e-9 {
        return overlap(quad_bounds(q), r).then_some(1.0);
    }
    let c = (clipped_area(&q.0, r) / a).min(1.0);
    (c >= TEXT_OVERLAP).then_some(c)
}

/// Whether the box `b` lies inside the union of `regions`: every cell of the grid that `b` and
/// the region edges cut it into has its centre in some region.
pub(super) fn covered(b: B, regions: &[B]) -> bool {
    let samples = |lo: f64, hi: f64, cuts: &mut Vec<f64>| {
        if lo >= hi {
            return vec![lo];
        }
        cuts.retain(|c| *c > lo && *c < hi);
        cuts.push(lo);
        cuts.push(hi);
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        cuts.windows(2).map(|w| (w[0] + w[1]) / 2.0).collect()
    };
    let xs = samples(
        b[0],
        b[2],
        &mut regions.iter().flat_map(|r| [r[0], r[2]]).collect(),
    );
    let ys = samples(
        b[1],
        b[3],
        &mut regions.iter().flat_map(|r| [r[1], r[3]]).collect(),
    );
    xs.iter().all(|&x| {
        ys.iter().all(|&y| {
            regions
                .iter()
                .any(|r| r[0] <= x && x <= r[2] && r[1] <= y && y <= r[3])
        })
    })
}

fn affine(m: Matrix) -> Affine {
    Affine::new(m.0)
}

pub(super) fn subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    for el in path.elements() {
        match el {
            PathEl::MoveTo(_) => out.push(BezPath::from_vec(vec![*el])),
            _ => {
                if let Some(last) = out.last_mut() {
                    last.push(*el);
                }
            }
        }
    }
    out
}

// ---- scanning ---------------------------------------------------------------------------------

/// A distinct glyph of a font: code, glyph id, Unicode.
type GlyphKey = (u32, Option<u32>, Option<String>);

/// Document-wide facts gathered while the pages are planned.
#[derive(Default)]
struct Scan {
    /// Each image XObject occurrence: (page, redacted).
    images: BTreeMap<ObjectId, Vec<(u32, bool)>>,
    /// Each Form object's occurrences: (page, chain of steps down to it).
    forms: BTreeMap<ObjectId, HashSet<(u32, Vec<InstanceStep>)>>,
    /// The (page, chain) occurrences a planned item is drawn through.
    changed: HashSet<(u32, Vec<InstanceStep>)>,
    streams: BTreeMap<ObjectId, BTreeSet<u32>>,
    survivors: BTreeMap<ObjectId, BTreeMap<GlyphKey, GlyphItem>>,
    touched_fonts: BTreeSet<ObjectId>,
}

impl Scan {
    fn note_item(&mut self, page: u32, loc: SourceLoc, path: &InstancePath) {
        self.streams.entry(loc.stream).or_default().insert(page);
        for n in 1..=path.0.len() {
            let chain = path.0[..n].to_vec();
            self.forms
                .entry(chain[n - 1].form)
                .or_default()
                .insert((page, chain));
        }
    }

    fn survivor(&mut self, g: &GlyphItem) {
        if let Some(font) = g.font {
            self.survivors
                .entry(font)
                .or_default()
                .entry((g.char_code, g.gid, g.unicode.clone()))
                .or_insert_with(|| g.clone());
        }
    }

    /// A page without regions: all of its content survives.
    fn untouched(&mut self, model: &SemanticPage) {
        let page = model.index as u32;
        for item in &model.items {
            self.note_item(page, item.loc, &item.path);
            match &item.kind {
                ItemKind::Glyph(g) => self.survivor(g),
                ItemKind::Image(i) => {
                    if let Some(obj) = i.obj {
                        self.images.entry(obj).or_default().push((page, false));
                    }
                }
                _ => {}
            }
        }
    }

    /// `program`: the embedded program each font draws from.
    fn finish(
        self,
        binding: Binding,
        mut pages: Vec<PagePlan>,
        program: &HashMap<ObjectId, ObjectId>,
    ) -> RedactionPlan {
        let shared = |form: ObjectId| self.forms.get(&form).is_some_and(|o| o.len() > 1);
        let mut report = RedactionReport {
            regions: 0,
            clusters_removed: 0,
            images_erased: 0,
            images_cloned: 0,
            forms_cloned: 0,
            vector_subpaths_removed: 0,
            vector_paths_cut: 0,
            collateral: Vec::new(),
            shared_survivors: Vec::new(),
        };
        let mut reencoded = BTreeSet::new();
        let mut image_pages: BTreeMap<ObjectId, BTreeSet<u32>> = BTreeMap::new();
        let mut form_pages: BTreeMap<ObjectId, BTreeSet<u32>> = BTreeMap::new();
        for p in &mut pages {
            report.regions += p.regions.len() as u32;
            report.clusters_removed += p.clusters.len() as u32;
            for img in &mut p.images {
                img.needs_clone = img
                    .obj
                    .is_some_and(|o| self.images.get(&o).is_some_and(|v| v.len() > 1));
                report.images_erased += 1;
                report.images_cloned += u32::from(img.needs_clone);
                reencoded.insert(p.page_index);
                if let Some(o) = img.obj {
                    image_pages.entry(o).or_default().insert(p.page_index);
                }
            }
            for f in &mut p.forms {
                f.shared = shared(f.form);
                report.forms_cloned += u32::from(f.shared);
                form_pages.entry(f.form).or_default().insert(p.page_index);
            }
            for v in &p.vectors {
                match v.class {
                    VectorClass::Cut => report.vector_paths_cut += 1,
                    VectorClass::Inside => report.vector_subpaths_removed += 1,
                }
            }
        }
        for (obj, redacted_on) in image_pages {
            let occ = &self.images[&obj];
            let untouched = pages_where(occ.iter().filter(|(_, r)| !r).map(|(p, _)| *p));
            if !untouched.is_empty() {
                report.shared_survivors.push(SharedSurvivor {
                    kind: SharedKind::Image,
                    redacted_on: redacted_on.into_iter().collect(),
                    untouched_on: untouched,
                });
            }
        }
        for (form, redacted_on) in form_pages {
            let untouched = pages_where(
                self.forms[&form]
                    .iter()
                    .filter(|o| !self.changed.contains(*o))
                    .map(|o| o.0),
            );
            if !untouched.is_empty() {
                report.shared_survivors.push(SharedSurvivor {
                    kind: SharedKind::Form,
                    redacted_on: redacted_on.into_iter().collect(),
                    untouched_on: untouched,
                });
            }
        }
        let mut losses = Vec::new();
        if report.images_erased > 0 {
            losses.push(Loss {
                code: LossCode::ImageReencoded,
                count: report.images_erased,
                pages: Some(reencoded.into_iter().collect()),
            });
        }
        let survivors = self.survivors;
        let removed_streams: BTreeSet<ObjectId> = pages
            .iter()
            .flat_map(|p| {
                let text = p.clusters.iter().map(|c| c.id.loc.stream);
                let images = p.images.iter().map(|i| i.loc.stream);
                text.chain(p.vectors.iter().map(|v| v.loc.stream))
                    .chain(images)
            })
            .collect();
        RedactionPlan {
            binding,
            font_survivors: self
                .touched_fonts
                .iter()
                .map(|f| {
                    let shared = program.get(f);
                    let kept = survivors
                        .iter()
                        .filter(|(g, _)| *g == f || (shared.is_some() && program.get(g) == shared))
                        .flat_map(|(_, glyphs)| glyphs.values().cloned())
                        .collect();
                    (*f, kept)
                })
                .collect(),
            stream_pages: self
                .streams
                .into_iter()
                .filter(|(s, _)| removed_streams.contains(s))
                .collect(),
            pages,
            report,
            losses,
        }
    }
}

fn pages_where(pages: impl Iterator<Item = u32>) -> Vec<u32> {
    pages.collect::<BTreeSet<_>>().into_iter().collect()
}

/// Every glyph of page `page` with its display-space bounds: how a cluster id of one file is
/// found again in a rewritten one (same geometry, new content-stream locations).
pub(crate) fn glyph_boxes(doc: &Document, page: usize) -> Result<Vec<(ClusterId, B)>, PdfError> {
    let model = doc.page_model(page)?;
    let mut out = Vec::new();
    for item in &model.items {
        if let ItemKind::Glyph(g) = &item.kind {
            let quad = g.quad_user.map(model.geometry.user_to_display);
            let id = ClusterId {
                page: page as u32,
                path: item.path.clone(),
                loc: item.loc,
                glyph: g.op_glyph_index,
            };
            out.push((id, quad_bounds(&quad)));
        }
    }
    Ok(out)
}

fn plan_page(
    doc: &Document,
    model: &SemanticPage,
    stored: &[&crate::StoredRedaction],
    scan: &mut Scan,
) -> Result<PagePlan, PdfError> {
    let page = model.index as u32;
    let to_display = affine(model.geometry.user_to_display);
    let disp_scale = to_display.determinant().abs().sqrt();
    let mut regions: Vec<PlannedRegion> = Vec::new();
    let mut boxes: Vec<B> = Vec::new();
    let mut must: HashMap<&ClusterId, Vec<u32>> = HashMap::new();
    for r in stored {
        let first = regions.len() as u32;
        for rect in &r.regions {
            boxes.push(bounds_of(rect));
            regions.push(PlannedRegion {
                redaction: r.id.clone(),
                requested: *rect,
                effective: *rect,
                fill: r.fill,
                clusters: 0,
                partial_clusters: 0,
                images: 0,
                vectors: 0,
            });
        }
        let own = (first..regions.len() as u32).collect::<Vec<_>>();
        for id in &r.must_remove {
            must.entry(id).or_default().extend(&own);
        }
    }
    let hits = |b: B| -> Vec<u32> {
        (0..boxes.len() as u32)
            .filter(|&i| overlap(b, boxes[i as usize]))
            .collect()
    };
    let pattern_locs: HashSet<SourceLoc> = model
        .diagnostics
        .iter()
        .filter(|d| d.kind == DiagnosticKind::PatternNotDescended)
        .filter_map(|d| d.loc)
        .collect();

    let mut plan = PagePlan {
        page_index: page,
        regions: Vec::new(),
        clusters: Vec::new(),
        images: Vec::new(),
        vectors: Vec::new(),
        forms: Vec::new(),
        annotations: Vec::new(),
        patterns: Vec::new(),
        pattern_regions: Vec::new(),
        soft_masks: Vec::new(),
    };
    let mut resolved: HashSet<&ClusterId> = HashSet::new();
    let mut form_seen: HashSet<Vec<InstanceStep>> = HashSet::new();
    let form_uses: HashMap<&InstancePath, SourceLoc> =
        model.forms.iter().map(|f| (&f.path, f.loc)).collect();
    let mut effective: Vec<B> = boxes.clone();
    let mut affect =
        |regions: &mut Vec<PlannedRegion>, hit: &[u32], b: B, f: fn(&mut PlannedRegion)| {
            for &i in hit {
                effective[i as usize] = union(effective[i as usize], b);
                f(&mut regions[i as usize]);
            }
        };

    for item in &model.items {
        scan.note_item(page, item.loc, &item.path);
        let mut changed = false;
        match &item.kind {
            ItemKind::Glyph(g) => {
                let quad = g.quad_user.map(model.geometry.user_to_display);
                let by_area: Vec<(u32, f64)> = (0..boxes.len() as u32)
                    .filter_map(|i| Some((i, text_coverage(&quad, boxes[i as usize])?)))
                    .collect();
                let id = ClusterId {
                    page,
                    path: item.path.clone(),
                    loc: item.loc,
                    glyph: g.op_glyph_index,
                };
                let forced = must.get_key_value(&id);
                if by_area.is_empty() && forced.is_none() {
                    scan.survivor(g);
                    continue;
                }
                let mut hit: Vec<u32> = by_area.iter().map(|(i, _)| *i).collect();
                if let Some((key, own)) = forced {
                    resolved.insert(*key);
                    // The regions of that redaction that reach the glyph, else all of them.
                    let near: Vec<u32> = own
                        .iter()
                        .copied()
                        .filter(|&i| overlap(quad_bounds(&quad), boxes[i as usize]))
                        .collect();
                    hit.extend(if near.is_empty() { own.clone() } else { near });
                    hit.sort_unstable();
                    hit.dedup();
                }
                let coverage = by_area.iter().map(|(_, c)| *c).fold(0.0, f64::max);
                let partial = !hit.is_empty() && coverage < 1.0 - 1e-6;
                let b = quad_bounds(&quad);
                affect(&mut regions, &hit, b, |r| r.clusters += 1);
                if partial {
                    for &i in &hit {
                        regions[i as usize].partial_clusters += 1;
                    }
                }
                scan.touched_fonts.extend(g.font);
                plan.clusters.push(PlannedCluster {
                    id,
                    font: g.font,
                    code: g.char_code,
                    quad,
                    reason: match (by_area.is_empty(), forced.is_some()) {
                        (false, true) => ClusterReason::Both,
                        (true, _) => ClusterReason::MustRemove,
                        _ => ClusterReason::Overlap,
                    },
                    coverage,
                    advance_em: g.advance_em,
                    multi_codepoint: g
                        .unicode
                        .as_deref()
                        .is_some_and(|u| u.chars().nth(1).is_some()),
                    invisible: matches!(g.render_mode, 3 | 7),
                    regions: hit,
                });
                changed = true;
            }
            ItemKind::Image(img) => {
                let unit = Quad(
                    [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(x, y)| Point { x, y }),
                );
                let quad = unit.map(img.ctm).map(model.geometry.user_to_display);
                let hit: Vec<u32> = (0..boxes.len() as u32)
                    .filter(|&i| clipped_area(&quad.0, boxes[i as usize]) > 1e-9)
                    .collect();
                if let Some(obj) = img.obj {
                    scan.images
                        .entry(obj)
                        .or_default()
                        .push((page, !hit.is_empty()));
                }
                if !hit.is_empty() {
                    affect(&mut regions, &hit, quad_bounds(&quad), |r| r.images += 1);
                    plan.images.push(PlannedImage {
                        loc: item.loc,
                        path: item.path.clone(),
                        obj: img.obj,
                        quad,
                        needs_clone: false,
                        regions: hit,
                    });
                    changed = true;
                }
            }
            ItemKind::Path(p)
                if (p.clip_only && clip_from_stream(item))
                    || (!p.clip_only && (p.fill.is_some() || p.stroke.is_some())) =>
            {
                let found = plan_vectors(item.loc, &item.path, p, to_display, disp_scale, &boxes);
                changed = !found.is_empty();
                for v in found {
                    // Cut exactly to the regions: nothing beyond them goes, so `effective` stays.
                    for &i in &v.regions {
                        regions[i as usize].vectors += 1;
                    }
                    plan.vectors.push(v);
                }
                if pattern_locs.contains(&item.loc) {
                    let b = bounds_of_path(&p.geometry, to_display);
                    let hit = hits(b);
                    if !hit.is_empty() {
                        plan.patterns.push(rect_of(b));
                        plan.pattern_regions.extend(hit);
                    }
                }
            }
            _ => {}
        }
        if changed {
            for n in 1..=item.path.0.len() {
                let chain = item.path.0[..n].to_vec();
                if form_seen.insert(chain.clone()) {
                    scan.changed.insert((page, chain.clone()));
                    let path = InstancePath(chain.clone());
                    let do_loc = form_uses
                        .get(&path)
                        .copied()
                        .ok_or(PdfError::at(ErrorCode::Internal, chain[n - 1].form))?;
                    plan.forms.push(PlannedForm {
                        path,
                        form: chain[n - 1].form,
                        shared: false,
                        do_loc,
                    });
                }
            }
        }
    }
    if let Some(missing) = must.keys().find(|id| !resolved.contains(*id)) {
        // The ids came from this revision's own text; one that names no glyph is a bug.
        return Err(PdfError::at(ErrorCode::Internal, missing.loc.stream));
    }
    let groups = model.soft_mask_groups.iter().map(|g| {
        let q = Quad(
            [(g[0], g[1]), (g[2], g[1]), (g[2], g[3]), (g[0], g[3])].map(|(x, y)| Point { x, y }),
        );
        quad_bounds(&q.map(model.geometry.user_to_display))
    });
    let masked = model
        .soft_masked
        .iter()
        .filter_map(|&i| model.items.get(i))
        .map(|it| item_bounds(it, model, to_display, disp_scale));
    for b in masked.chain(groups) {
        plan.soft_masks.extend(hits(b));
    }
    plan.soft_masks.sort_unstable();
    plan.soft_masks.dedup();
    plan.annotations = annotations(doc, model, &boxes)?;
    for (r, e) in regions.iter_mut().zip(effective) {
        r.effective = rect_of(e);
    }
    plan.regions = regions;
    Ok(plan)
}

/// Display-space bounds of an item, strokes grown by their full width (a bound, not the exact reach).
fn item_bounds(item: &Item, model: &SemanticPage, to_display: Affine, disp_scale: f64) -> B {
    let u2d = model.geometry.user_to_display;
    match &item.kind {
        ItemKind::Glyph(g) => quad_bounds(&g.quad_user.map(u2d)),
        ItemKind::Image(i) => {
            let unit =
                Quad([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(x, y)| Point { x, y }));
            quad_bounds(&unit.map(i.ctm).map(u2d))
        }
        ItemKind::Path(p) | ItemKind::Shading(p) => {
            let b = bounds_of_path(&p.geometry, to_display);
            let g = p.stroke.as_ref().map_or(0.0, |s| {
                f64::from(s.line_width) * p.stroke_scale * disp_scale
            });
            [b[0] - g, b[1] - g, b[2] + g, b[3] + g]
        }
    }
}

fn bounds_of_path(path: &BezPath, to_display: Affine) -> B {
    let r = (to_display * path.clone()).bounding_box();
    [r.x0, r.y0, r.x1, r.y1]
}

/// A clip-only item written by a `W n` of the stream it names, not the BBox clip of a Form or
/// pattern (the interpreter reports those at the `Do` that draws them, in the parent's stream).
fn clip_from_stream(item: &Item) -> bool {
    item.loc.stream.num != 0 && item.path.0.last().is_none_or(|s| s.form == item.loc.stream)
}

fn plan_vectors(
    loc: SourceLoc,
    path: &InstancePath,
    item: &PathItem,
    to_display: Affine,
    disp_scale: f64,
    boxes: &[B],
) -> Vec<PlannedVector> {
    // One unit per painted path: fill rules act across subpaths and a stroke's ink is the union
    // of its subpaths', so only the geometry stage can tell what a cut leaves.
    let r = (to_display * item.geometry.clone()).bounding_box();
    let mut b = [r.x0, r.y0, r.x1, r.y1];
    if let Some(s) = &item.stroke {
        // Half the width times the miter limit (spec §5.4), at least the square-cap corner, along
        // the CTM's widest axis (`stroke_scale`): a bound, never the verdict.
        let cap = if matches!(s.line_cap, Cap::Square) {
            std::f64::consts::SQRT_2
        } else {
            1.0
        };
        let grow = f64::from(s.line_width)
            * item.stroke_scale
            * disp_scale
            * 0.5
            * f64::from(s.miter_limit).max(1.0).max(cap);
        b = [b[0] - grow, b[1] - grow, b[2] + grow, b[3] + grow];
    }
    let hit: Vec<u32> = (0..boxes.len() as u32)
        .filter(|&i| overlap(b, boxes[i as usize]))
        .collect();
    if hit.is_empty() {
        return Vec::new();
    }
    let class = if !item.clip_only && covered(b, boxes) {
        VectorClass::Inside
    } else {
        VectorClass::Cut
    };
    let kind = match (item.clip_only, item.stroke.is_some()) {
        (true, _) => VectorKind::Clip,
        (_, true) => VectorKind::Stroke,
        _ => VectorKind::Fill,
    };
    vec![PlannedVector {
        loc,
        path: path.clone(),
        kind,
        class,
        bounds: rect_of(b),
        regions: hit,
    }]
}

/// Annotations of the page whose /Rect (mapped to display space) meets a region.
fn annotations(
    doc: &Document,
    model: &SemanticPage,
    boxes: &[B],
) -> Result<Vec<PlannedAnnotation>, PdfError> {
    let page = doc.reader().page(model.index)?;
    let Some(list) = page.raw().get::<Array<'_>>("Annots") else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for a in list.iter::<Dict<'_>>() {
        let Some(rect) = a.get::<Array<'_>>("Rect") else {
            continue;
        };
        let v: Vec<f64> = rect.iter::<f64>().collect();
        let [x0, y0, x1, y1] = v[..] else { continue };
        let q = Quad([(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| Point { x, y }))
            .map(model.geometry.user_to_display);
        let b = quad_bounds(&q);
        let regions: Vec<u32> = (0..boxes.len() as u32)
            .filter(|&i| overlap(b, boxes[i as usize]))
            .collect();
        if !regions.is_empty() {
            out.push(PlannedAnnotation {
                obj: a.obj_id().map(|id| crate::reader::object_id(id.into())),
                subtype: a
                    .get::<Name<'_>>("Subtype")
                    .map(|n| n.as_str().to_owned())
                    .unwrap_or_default(),
                bounds: rect_of(b),
                regions,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn rotated_quad_overlap_is_the_clipped_share() {
        // A 10 x 10 square turned 45 degrees (diamond), the right half inside x >= 0.
        let d = [
            p(0.0, -7.0711),
            p(7.0711, 0.0),
            p(0.0, 7.0711),
            p(-7.0711, 0.0),
        ];
        let c = clipped_area(&d, [0.0, -100.0, 100.0, 100.0]) / area(&d);
        assert!((c - 0.5).abs() < 1e-3);
        assert!(text_coverage(&Quad(d), [-100.0, -100.0, -7.0, 100.0]).is_none());
    }

    #[test]
    fn union_cover_needs_every_cell() {
        let two = [[0.0, 0.0, 10.0, 10.0], [10.0, 0.0, 20.0, 10.0]];
        assert!(covered([2.0, 2.0, 18.0, 8.0], &two));
        assert!(!covered([2.0, 2.0, 18.0, 12.0], &two));
        assert!(!covered(
            [2.0, 2.0, 18.0, 8.0],
            &[two[0], [12.0, 0.0, 20.0, 10.0]]
        ));
    }
}
