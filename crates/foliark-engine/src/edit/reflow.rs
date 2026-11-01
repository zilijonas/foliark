//! Opt-in push-down (ADR 0015 Decision B): an edit that needs more lines than `maxWrapLines`
//! moves the content below the paragraph down by `dy` instead of refusing.
//!
//! The moved set is geometric (display space, from the page model): every item at or below the
//! cut that overlaps the paragraph's column, the column grown to what moves, walking down until
//! a gap of at least `dy + 0.5 em`. The content stream is then cut into units (`q … Q` groups
//! and leaves: `BT … ET`, path objects, `Do`, `sh`, inline images) and whole units are translated
//! by `cm`: right after the `q` of a group whose items all move, else around a leaf. Anything a
//! translation of whole units cannot move exactly is a typed refusal, never a partial move.
use super::paragraph::Prepared;
use crate::Document;
use crate::interpret::{DiagnosticKind, ItemKind};
use crate::reader::{ContentStream, object_id};
use crate::redaction::plan::display_bounds;
use crate::redaction::text::num;
use crate::writer::{Val, Writer, instruction};
use foliark_model::{
    InstancePath, Matrix, ObjectId, PageShift, PdfError, PushDown, Rect, SourceLoc,
};
use hayro::hayro_syntax::content::UntypedIter;
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

type B = [f64; 4];

/// Display-space slack for edges computed twice from the same model.
const EPS: f64 = 0.05;

/// A refusal: `Some(feature)` for `EDIT_NO_FIT.details.feature`, `None` for a plain overflow.
pub(crate) type Refusal = Option<&'static str>;

const SHARED_BLOCK: &str = "reflowSharedBlock";
const SPLITS_FORM: &str = "reflowSplitsForm";
const STRADDLE: &str = "reflowStraddle";
const CLIPPED: &str = "reflowClipped";
const SOFT_MASK: &str = "reflowSoftMask";
const PATTERN: &str = "reflowPattern";
const SHARED_CONTENT: &str = "reflowSharedContent";
const ANNOTATION: &str = "reflowAnnotation";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Group,
    Text,
    Path,
    /// `Do`.
    Form,
    /// `sh`.
    Shading,
    /// `BI … EI` (one instruction).
    Image,
}

/// Position in the page's content: (index of the stream in `/Contents`, byte offset).
type Pos = (usize, usize);

struct Unit {
    kind: Kind,
    start: Pos,
    /// `None` while open; a unit still open at the end of the content is never moved whole.
    end: Option<Pos>,
    /// The first and last instruction (stream index, span).
    first: (usize, Range<usize>),
    last: (usize, Range<usize>),
    /// The enclosing group.
    parent: Option<usize>,
    /// The CTM where the unit starts.
    ctm: [f64; 6],
    /// A path with `W`/`W*`, or text drawn in a clipping mode (4-7).
    clip: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    /// A glyph the edit removes: it neither moves nor stays.
    Removed,
    /// Drawn by an annotation's appearance: annotations are judged by their `/Rect`.
    Annotation,
    /// Clip-only geometry: it paints nothing.
    Clip,
    Drawn,
}

/// What `plan` decided for one `dy`.
pub(crate) struct Moved {
    dy: f64,
    /// The moved region (display space, before the move).
    region: B,
    groups: Vec<usize>,
    leaves: Vec<usize>,
    /// Link annotations to move.
    links: Vec<ObjectId>,
}

/// A planned push-down, ready to write.
pub(crate) struct Push {
    pub shift: PageShift,
    spans: Vec<(ObjectId, Range<usize>, Vec<u8>)>,
    links: Vec<(ObjectId, &'static str, Val)>,
}

pub(crate) struct Reflow<'a> {
    doc: &'a Document,
    page: usize,
    p: &'a Prepared,
    streams: Vec<ContentStream>,
    /// Streams of this page that `/Contents` of any page (this one included) names more than once.
    shared: HashSet<ObjectId>,
    units: Vec<Unit>,
    /// Indices into `units` of the leaves, in content order.
    leaves: Vec<usize>,
    bounds: Vec<B>,
    class: Vec<Class>,
    /// The leaf each item is drawn by (through a page-level `Do` for Form content).
    leaf: Vec<Option<usize>>,
    /// Drawn items at or below the cut, by top.
    below: Vec<usize>,
    cut: f64,
}

fn overlaps(a: B, b: B) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

fn contains(outer: B, inner: B) -> bool {
    outer[0] <= inner[0] + EPS
        && outer[1] <= inner[1] + EPS
        && outer[2] >= inner[2] - EPS
        && outer[3] >= inner[3] - EPS
}

fn union(a: B, b: B) -> B {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

fn down(b: B, dy: f64) -> B {
    [b[0], b[1] + dy, b[2], b[3] + dy]
}

/// PDF `cm` composition: `m` then `ctm`.
fn mul(m: [f64; 6], ctm: [f64; 6]) -> [f64; 6] {
    let [a, b, c, d, e, f] = m;
    let [a2, b2, c2, d2, e2, f2] = ctm;
    [
        a * a2 + b * c2,
        a * b2 + b * d2,
        c * a2 + d * c2,
        c * b2 + d * d2,
        e * a2 + f * c2 + e2,
        e * b2 + f * d2 + f2,
    ]
}

fn transform(m: &Matrix, (x, y): (f64, f64)) -> (f64, f64) {
    let [a, b, c, d, e, f] = m.0;
    (a * x + c * y + e, b * x + d * y + f)
}

fn numbers(a: &Array<'_>) -> Option<Vec<f64>> {
    let v: Vec<f64> = a.iter::<f64>().collect();
    v.iter().all(|x| x.is_finite()).then_some(v)
}

/// The content of page `page` cut into units.
fn scan(streams: &[ContentStream]) -> Vec<Unit> {
    const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut units: Vec<Unit> = Vec::new();
    let mut groups: Vec<usize> = Vec::new();
    let mut ctms: Vec<[f64; 6]> = Vec::new();
    let mut ctm = IDENTITY;
    let (mut text, mut path): (Option<usize>, Option<usize>) = (None, None);
    for (si, cs) in streams.iter().enumerate() {
        let mut it = UntypedIter::new(&cs.data);
        while let Some((ins, span)) = it.next_with_span() {
            let op: &[u8] = ins.operator.as_ref();
            let here = (si, span.clone());
            let new = |kind, parent: Option<&usize>, ctm| Unit {
                kind,
                start: (si, span.start),
                end: None,
                first: here.clone(),
                last: here.clone(),
                parent: parent.copied(),
                ctm,
                clip: false,
            };
            let close = |u: &mut Unit| {
                u.end = Some((si, span.end));
                u.last = here.clone();
            };
            match op {
                b"q" if text.is_none() && path.is_none() => {
                    let g = push(&mut units, new(Kind::Group, groups.last(), ctm));
                    groups.push(g);
                    ctms.push(ctm);
                }
                b"Q" if text.is_none() && path.is_none() => {
                    // An unmatched `Q` restores nothing the page set.
                    if let Some(g) = groups.pop() {
                        close(&mut units[g]);
                        ctm = ctms.pop().unwrap_or(IDENTITY);
                    }
                }
                b"cm" if text.is_none() && path.is_none() => {
                    let v: Vec<f64> = ins
                        .operands()
                        .filter_map(|o| match o {
                            Object::Number(n) => Some(n.as_f64()),
                            _ => None,
                        })
                        .collect();
                    if let Ok(m) = <[f64; 6]>::try_from(v) {
                        ctm = mul(m, ctm);
                    }
                }
                b"BT" if text.is_none() => {
                    text = Some(push(&mut units, new(Kind::Text, groups.last(), ctm)))
                }
                b"ET" => {
                    if let Some(t) = text.take() {
                        close(&mut units[t]);
                    }
                }
                b"m" | b"re" if text.is_none() && path.is_none() => {
                    path = Some(push(&mut units, new(Kind::Path, groups.last(), ctm)));
                }
                b"W" | b"W*" => {
                    if let Some(p) = path {
                        units[p].clip = true;
                    }
                }
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                    if let Some(p) = path.take() {
                        close(&mut units[p]);
                    }
                }
                b"Do" | b"sh" | b"BI" if text.is_none() && path.is_none() => {
                    let kind = match op {
                        b"Do" => Kind::Form,
                        b"sh" => Kind::Shading,
                        _ => Kind::Image,
                    };
                    let mut u = new(kind, groups.last(), ctm);
                    close(&mut u);
                    push(&mut units, u);
                }
                _ => {}
            }
        }
    }
    units
}

fn push(units: &mut Vec<Unit>, u: Unit) -> usize {
    units.push(u);
    units.len() - 1
}

/// Every content stream `/Contents` of `doc`'s pages names more than once.
fn shared_streams(doc: &Document) -> HashSet<ObjectId> {
    let mut seen = HashSet::new();
    let mut twice = HashSet::new();
    for p in doc.reader().pdf().pages().iter() {
        let refs: Vec<ObjectId> = match p.raw().get_raw::<Object<'_>>("Contents") {
            Some(MaybeRef::Ref(r)) => match p.raw().get::<Array<'_>>("Contents") {
                Some(a) => a
                    .raw_iter()
                    .filter_map(|i| i.as_obj_ref())
                    .map(object_id)
                    .collect(),
                None => vec![object_id(r)],
            },
            Some(MaybeRef::NotRef(Object::Array(a))) => a
                .raw_iter()
                .filter_map(|i| i.as_obj_ref())
                .map(object_id)
                .collect(),
            _ => Vec::new(),
        };
        for r in refs {
            if !seen.insert(r) {
                twice.insert(r);
            }
        }
    }
    twice
}

impl<'a> Reflow<'a> {
    pub fn new(doc: &'a Document, page: usize, p: &'a Prepared) -> Result<Self, PdfError> {
        let model = &p.model;
        let streams = doc.reader().page_contents(page)?;
        let units = scan(&streams);
        let leaves: Vec<usize> = (0..units.len())
            .filter(|&u| units[u].kind != Kind::Group)
            .collect();
        let index: HashMap<ObjectId, usize> = streams
            .iter()
            .enumerate()
            .rev()
            .map(|(i, s)| (s.obj, i))
            .collect();
        let find = |loc: &SourceLoc| -> Option<usize> {
            let si = *index.get(&loc.stream)?;
            let at = leaves.partition_point(|&u| units[u].start <= (si, loc.start as usize));
            let u = *leaves.get(at.checked_sub(1)?)?;
            units[u]
                .end
                .is_some_and(|e| (si, loc.end as usize) <= e)
                .then_some(u)
        };
        // Form content is placed by the page-level `Do` that draws it.
        let dos: HashMap<_, SourceLoc> = model
            .forms
            .iter()
            .filter(|f| f.path.0.len() == 1)
            .map(|f| (f.path.0[0], f.loc))
            .collect();
        let removed: HashSet<(&InstancePath, SourceLoc, u32)> = p
            .ids
            .iter()
            .map(|id| (&id.path, id.loc, id.glyph))
            .collect();
        let bounds = display_bounds(model);
        let mut class = Vec::with_capacity(model.items.len());
        let mut leaf = Vec::with_capacity(model.items.len());
        for it in &model.items {
            let (c, l) = match (&it.kind, it.path.0.first()) {
                (ItemKind::Path(path), _) if path.clip_only => (Class::Clip, find(&it.loc)),
                (ItemKind::Glyph(g), _)
                    if removed.contains(&(&it.path, it.loc, g.op_glyph_index)) =>
                {
                    (Class::Removed, None)
                }
                (_, None) => (Class::Drawn, find(&it.loc)),
                (_, Some(step)) => match dos.get(step) {
                    Some(loc) if index.contains_key(&loc.stream) => (Class::Drawn, find(loc)),
                    _ => (Class::Annotation, None),
                },
            };
            class.push(c);
            leaf.push(l);
        }
        let mut units = units;
        for (i, it) in model.items.iter().enumerate() {
            if let (ItemKind::Glyph(g), Some(u)) = (&it.kind, leaf[i])
                && g.render_mode >= 4
                && class[i] != Class::Annotation
                && it.path.0.is_empty()
            {
                units[u].clip = true;
            }
        }
        let cut = p
            .limits
            .bottom_limit(p.info.origin_x + p.info.max_wrap_width);
        let mut below: Vec<usize> = (0..bounds.len())
            .filter(|&i| {
                class[i] == Class::Drawn
                    && bounds[i].iter().all(|v| v.is_finite())
                    && bounds[i][1] >= cut - EPS
            })
            .collect();
        below.sort_by(|&a, &b| bounds[a][1].total_cmp(&bounds[b][1]).then(a.cmp(&b)));
        let shared = shared_streams(doc)
            .into_iter()
            .filter(|s| index.contains_key(s))
            .chain(
                // A stream named twice by this page's own `/Contents` array.
                streams
                    .iter()
                    .enumerate()
                    .filter(|(i, s)| index[&s.obj] != *i)
                    .map(|(_, s)| s.obj),
            )
            .collect();
        Ok(Self {
            doc,
            page,
            p,
            streams,
            shared,
            units,
            leaves,
            bounds,
            class,
            leaf,
            below,
            cut,
        })
    }

    fn sel(&self) -> B {
        let b = self.p.info.bounds;
        [b.x, b.y, b.x + b.width, b.y + b.height]
    }

    /// The moved set for `dy` with every check, or the refusal.
    pub fn plan(&self, dy: f64) -> Result<Moved, Refusal> {
        let info = &self.p.info;
        let b = &self.bounds;
        let em = info.font_size;
        // The column grows to what moves until stable; walking down by top, a gap of at least
        // `dy + 0.5 em` ends the moved set (a footer below it stays).
        let mut col = (info.origin_x, info.origin_x + info.max_wrap_width);
        let (items, stop) = loop {
            let (mut set, mut bottom, mut stop) = (Vec::new(), self.cut, None);
            for (k, &i) in self.below.iter().enumerate() {
                if !(b[i][2] > col.0 && b[i][0] < col.1) {
                    continue;
                }
                if b[i][1] - bottom >= dy + 0.5 * em {
                    stop = Some(k);
                    break;
                }
                set.push(i);
                bottom = bottom.max(b[i][3]);
            }
            let grown = set
                .iter()
                .fold(col, |c, &i| (c.0.min(b[i][0]), c.1.max(b[i][2])));
            if grown == col {
                break (set, stop);
            }
            col = grown;
        };
        if items.is_empty() {
            return Err(None);
        }
        let moving: HashSet<usize> = items.iter().copied().collect();
        let region = items.iter().fold(b[items[0]], |r, &i| union(r, b[i]));
        let region = [col.0, region[1], col.1, region[3]];
        let in_col = |i: usize| b[i][2] > col.0 && b[i][0] < col.1;

        // The bound: the next fixed item below, the page's text margin, containers.
        let mut limit = self.p.limits.text_bottom.min(self.p.limits.page_h);
        if let Some(k) = stop {
            for &i in &self.below[k..] {
                if in_col(i) && !moving.contains(&i) {
                    limit = limit.min(b[i][1]);
                }
            }
        }
        let sel = self.sel();
        for (i, &bi) in b.iter().enumerate() {
            if self.class[i] != Class::Drawn || moving.contains(&i) || !in_col(i) {
                continue;
            }
            if bi[1] < self.cut - EPS && bi[3] > self.cut + EPS {
                if contains(bi, sel) && contains(bi, region) {
                    limit = limit.min(bi[3]);
                } else {
                    return Err(Some(STRADDLE));
                }
            }
        }
        if region[3] + dy > limit + EPS {
            return Err(None);
        }

        // Structure: what moves must be whole units.
        let model = &self.p.model;
        let mut count = vec![(0u32, 0u32); self.units.len()];
        for i in 0..b.len() {
            if self.class[i] != Class::Drawn {
                continue;
            }
            let Some(mut u) = self.leaf[i] else {
                if moving.contains(&i) {
                    return Err(Some(SHARED_BLOCK));
                }
                continue;
            };
            loop {
                if moving.contains(&i) {
                    count[u].0 += 1;
                } else {
                    count[u].1 += 1;
                }
                match self.units[u].parent {
                    Some(g) => u = g,
                    None => break,
                }
            }
        }
        for &i in &items {
            if model.soft_masked.contains(&i) {
                return Err(Some(SOFT_MASK));
            }
            let pattern = match &model.items[i].kind {
                ItemKind::Glyph(g) => g.fill.is_none(),
                ItemKind::Shading(_) => {
                    self.leaf[i].is_none_or(|u| self.units[u].kind != Kind::Shading)
                        || !model.items[i].path.0.is_empty()
                }
                _ => false,
            };
            let loc = model.items[i].loc;
            if pattern
                || model
                    .diagnostics
                    .iter()
                    .any(|d| d.kind == DiagnosticKind::PatternNotDescended && d.loc == Some(loc))
            {
                return Err(Some(PATTERN));
            }
        }
        // Outermost groups whose items all move, then the moving leaves outside them.
        let mut covered = vec![false; self.units.len()];
        let mut groups = Vec::new();
        for (g, u) in self.units.iter().enumerate() {
            let above = u.parent.is_some_and(|p| covered[p]);
            if u.kind == Kind::Group
                && !above
                && u.end.is_some()
                && count[g].0 > 0
                && count[g].1 == 0
            {
                covered[g] = true;
                groups.push(g);
            } else {
                covered[g] = above;
            }
        }
        let mut leaves = Vec::new();
        for &l in &self.leaves {
            if covered[l] || count[l].0 == 0 {
                continue;
            }
            let u = &self.units[l];
            if count[l].1 > 0 {
                return Err(Some(if u.kind == Kind::Form {
                    SPLITS_FORM
                } else {
                    SHARED_BLOCK
                }));
            }
            if u.clip {
                return Err(Some(CLIPPED));
            }
            if u.end.is_none() {
                return Err(Some(SHARED_BLOCK));
            }
            leaves.push(l);
        }
        for &u in groups.iter().chain(&leaves) {
            let unit = &self.units[u];
            if self.shared.contains(&self.streams[unit.first.0].obj)
                || self.shared.contains(&self.streams[unit.last.0].obj)
            {
                return Err(Some(SHARED_CONTENT));
            }
            self.clips_allow(u, &items, dy)?;
        }
        let links = self.links(&moving, region, dy)?;
        Ok(Moved {
            dy,
            region,
            groups,
            leaves,
            links,
        })
    }

    /// Every clip in force where unit `u` starts and set outside it must be an axis-aligned
    /// rectangle holding `u`'s moving items before and after the move.
    fn clips_allow(&self, u: usize, items: &[usize], dy: f64) -> Result<(), Refusal> {
        let ancestors: Vec<usize> =
            std::iter::successors(self.units[u].parent, |&g| self.units[g].parent).collect();
        let mine = items
            .iter()
            .filter(|&&i| {
                std::iter::successors(self.leaf[i], |&x| self.units[x].parent).any(|x| x == u)
            })
            .fold(None, |r: Option<B>, &i| {
                Some(r.map_or(self.bounds[i], |r| union(r, self.bounds[i])))
            });
        let Some(mine) = mine else { return Ok(()) };
        let start = self.units[u].start;
        for (c, clip) in self.units.iter().enumerate() {
            let active = clip.clip
                && clip.end.is_some_and(|e| e <= start)
                && clip.parent.is_none_or(|p| ancestors.contains(&p));
            if !active {
                continue;
            }
            if clip.kind != Kind::Path {
                return Err(Some(CLIPPED));
            }
            // The clip-only item the interpreter reported for this path.
            let rect = (0..self.bounds.len()).find(|&i| {
                self.class[i] == Class::Clip
                    && self.leaf[i] == Some(c)
                    && self.p.model.items[i].path.0.is_empty()
            });
            let ok = rect.is_some_and(|i| {
                let ItemKind::Path(p) = &self.p.model.items[i].kind else {
                    return false;
                };
                let b = self.bounds[i];
                let to_display = {
                    let [a, b, c, d, e, f] = self.p.model.geometry.user_to_display.0;
                    hayro::vello_cpu::peniko::kurbo::Affine::new([a, b, c, d, e, f])
                };
                use hayro::vello_cpu::peniko::kurbo::{PathEl, Shape};
                let path = to_display * p.geometry.clone();
                let lines = path.elements().iter().all(|e| {
                    matches!(e, PathEl::MoveTo(_) | PathEl::LineTo(_) | PathEl::ClosePath)
                });
                let moves = path
                    .elements()
                    .iter()
                    .filter(|e| matches!(e, PathEl::MoveTo(_)))
                    .count();
                let area = (b[2] - b[0]) * (b[3] - b[1]);
                lines
                    && moves == 1
                    && (path.area().abs() - area).abs() <= 1e-6 * area.max(1.0)
                    && contains(b, mine)
                    && contains(b, down(mine, dy))
            });
            if !ok {
                return Err(Some(CLIPPED));
            }
        }
        Ok(())
    }

    /// Link annotations wholly over moved content move with it; any other annotation that meets
    /// the moved area (before or after the move) refuses.
    fn links(&self, moving: &HashSet<usize>, region: B, dy: f64) -> Result<Vec<ObjectId>, Refusal> {
        let area = union(region, down(region, dy));
        let reader = self.doc.reader();
        let Ok(page) = reader.page(self.page) else {
            return Ok(Vec::new());
        };
        let Some(list) = page.raw().get::<Array<'_>>("Annots") else {
            return Ok(Vec::new());
        };
        let u2d = &self.p.model.geometry.user_to_display;
        let mut out = Vec::new();
        for entry in list.raw_iter() {
            let (id, dict) = match entry {
                MaybeRef::Ref(r) => (
                    Some(object_id(r)),
                    reader.pdf().xref().get::<Dict<'_>>(r.into()),
                ),
                MaybeRef::NotRef(Object::Dict(d)) => (None, Some(d)),
                MaybeRef::NotRef(_) => (None, None),
            };
            let Some(dict) = dict else { continue };
            let Some([x0, y0, x1, y1]) = dict
                .get::<Array<'_>>("Rect")
                .and_then(|a| numbers(&a))
                .and_then(|v| <[f64; 4]>::try_from(v).ok())
            else {
                continue;
            };
            let corners = [(x0, y0), (x1, y1), (x0, y1), (x1, y0)].map(|c| transform(u2d, c));
            let r = corners.iter().fold(
                [
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ],
                |r, &(x, y)| [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)],
            );
            if !overlaps(r, area) {
                continue;
            }
            let link = dict
                .get::<Name<'_>>("Subtype")
                .is_some_and(|n| n.as_str() == "Link");
            let meets = |moved: bool| {
                (0..self.bounds.len()).any(|i| {
                    self.class[i] == Class::Drawn
                        && moving.contains(&i) == moved
                        && overlaps(self.bounds[i], r)
                        && !contains(self.bounds[i], r)
                })
            };
            match id {
                Some(id) if link && meets(true) && !meets(false) => out.push(id),
                _ => return Err(Some(ANNOTATION)),
            }
        }
        Ok(out)
    }

    /// `ParagraphInfo.pushDown`: the most lines push-down allows at `maxWrapWidth`.
    pub fn budget(&self) -> Option<PushDown> {
        let info = &self.p.info;
        let mut best = None;
        let mut k = 1;
        while f64::from(k) * info.line_step <= self.p.limits.page_h {
            let Ok(m) = self.plan(f64::from(k) * info.line_step) else {
                break;
            };
            best = Some(PushDown {
                max_lines: info.max_wrap_lines + k,
                region: rect(m.region),
            });
            k += 1;
        }
        best
    }

    /// The push for `needed` lines (more than `maxWrapLines`).
    pub fn push(&self, needed: u32) -> Result<Push, Refusal> {
        let info = &self.p.info;
        let dy = f64::from(needed - info.max_wrap_lines) * info.line_step;
        // The paragraph's own last line must end above where the moved content starts now.
        let last = info.first_baseline + f64::from(needed - 1) * info.line_step + self.p.descent;
        if last > self.cut + dy + 0.1 || last > self.p.limits.container_bottom + 0.1 {
            return Err(None);
        }
        let m = self.plan(dy)?;
        self.emit(&m)
    }

    fn emit(&self, m: &Moved) -> Result<Push, Refusal> {
        let geometry = &self.p.model.geometry;
        let [_, _, c, d, _, _] = geometry.display_to_user.0;
        let (vx, vy) = (c * m.dy, d * m.dy);
        let neg = |s: &str| match s.strip_prefix('-') {
            Some(p) => p.to_owned(),
            None if s == "0" => s.to_owned(),
            None => format!("-{s}"),
        };
        let mut spans = Vec::new();
        let op = |at: &(usize, Range<usize>)| instruction(&self.streams[at.0].data[at.1.clone()]);
        for &u in m.groups.iter().chain(&m.leaves) {
            let unit = &self.units[u];
            let [a, b, c, d, _, _] = unit.ctm;
            let inv = Matrix([a, b, c, d, 0.0, 0.0])
                .inverse()
                .ok_or(Some(SHARED_BLOCK))?
                .0;
            let (tx, ty) = (inv[0] * vx + inv[2] * vy, inv[1] * vx + inv[3] * vy);
            let (tx, ty) = (num(tx, 5), num(ty, 5));
            let cm = format!("1 0 0 1 {tx} {ty} cm");
            let back = format!("1 0 0 1 {} {} cm", neg(&tx), neg(&ty));
            let obj = |at: &(usize, Range<usize>)| self.streams[at.0].obj;
            if unit.kind == Kind::Group {
                let q = format!("q\n{cm}").into_bytes();
                spans.push((obj(&unit.first), unit.first.1.clone(), q));
            } else if unit.first == unit.last {
                let mut s = format!("{cm}\n").into_bytes();
                s.extend(op(&unit.first));
                s.extend(format!("\n{back}").bytes());
                spans.push((obj(&unit.first), unit.first.1.clone(), s));
            } else {
                let mut s = format!("{cm}\n").into_bytes();
                s.extend(op(&unit.first));
                spans.push((obj(&unit.first), unit.first.1.clone(), s));
                let mut e = op(&unit.last);
                e.extend(format!("\n{back}").bytes());
                spans.push((obj(&unit.last), unit.last.1.clone(), e));
            }
        }
        // Annotation rectangles are in the page's default user space.
        let mut links = Vec::new();
        let reader = self.doc.reader();
        for &id in &m.links {
            let ident = hayro::hayro_syntax::object::ObjectIdentifier::new(
                id.num as i32,
                i32::from(id.generation),
            );
            let Some(dict) = reader.pdf().xref().get::<Dict<'_>>(ident) else {
                return Err(Some(ANNOTATION));
            };
            let shifted = |key: &str| {
                let v = dict.get::<Array<'_>>(key).and_then(|a| numbers(&a))?;
                Some(Val::Arr(
                    v.iter()
                        .enumerate()
                        .map(|(k, x)| Val::Real((x + if k % 2 == 0 { vx } else { vy }) as f32))
                        .collect(),
                ))
            };
            links.push((id, "Rect", shifted("Rect").ok_or(Some(ANNOTATION))?));
            if dict.contains_key("QuadPoints") {
                links.push((
                    id,
                    "QuadPoints",
                    shifted("QuadPoints").ok_or(Some(ANNOTATION))?,
                ));
            }
        }
        Ok(Push {
            shift: PageShift {
                page_index: self.page as u32,
                region: rect(m.region),
                dy: m.dy,
            },
            spans,
            links,
        })
    }
}

fn rect(b: B) -> Rect {
    Rect {
        x: b[0],
        y: b[1],
        width: b[2] - b[0],
        height: b[3] - b[1],
    }
}

impl Push {
    /// Write the translation into `w`, the writer of the edit that needs it.
    pub fn write(&self, w: &mut Writer<'_>) {
        for (stream, range, ops) in &self.spans {
            w.replace_span(*stream, range.clone(), ops);
        }
        for (id, key, val) in &self.links {
            w.override_entry(*id, key, val.clone());
        }
    }
}
