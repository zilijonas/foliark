//! Optional content and visibility (spec §8.2, HDDN-107): what the file's default configuration
//! hides, and what no viewer would show, is removed for real before anything is written.
//!
//! - Layers: `/OCProperties /D` (BaseState, ON, OFF, `/AS` View usage), OCG `/Intent`, OCMD
//!   `/P` and `/VE`. Content in `/OC name BDC ... EMC` that is OFF loses its drawing operators
//!   (text shows become a bare advance, so later text does not move; paths become `n`, so a
//!   pending clip survives); state operators stay. Every OC `BDC`/`EMC` pair is dropped, ON or
//!   OFF, with the OCG and OCMD entries of `/Properties` (writer, [`Writer::resolve_layers`]).
//!   An XObject whose `/OC` is OFF loses its `Do`; an annotation whose `/OC` is OFF is not
//!   flattened ([`super::interactive`]). Images and Forms that nothing draws any more are
//!   replaced by an empty Form, so their payload is not in the file.
//! - Off-page and clipped-out content: a text show, painted path, shading or image whose every
//!   occurrence lies entirely outside the CropBox or outside its effective clip (the bounds of
//!   the intersected clips, so only content certainly invisible goes) is removed the same way.
//!
//! Limits, named in the receipt: a show operator with any visible glyph is kept whole; white on
//! white and content covered by later opaque drawing are not detected; fonts used only by removed
//! text keep their programs and ToUnicode; an OFF object used by a pattern cell or Type3 glyph
//! procedure (not descended) refuses; a redaction that rewrites an operator this stage also
//! rewrites is refused by the writer (overlapping spans).
use super::text_semantics;
use crate::export::preflight::Blocker;
use crate::interpret::{DiagnosticKind, ItemKind};
use crate::limits::{self, Budget};
use crate::reader::{Document, object_id};
use crate::writer::{NewStream, Spans, Val, Writer};
use foliark_model::{ErrorCode, ObjectId, PdfError, Point};
use hayro::hayro_syntax::content::{Instruction, UntypedIter};
use hayro::hayro_syntax::object::{
    Array, Dict, MaybeRef, Name, Object, ObjectIdentifier as Oid, Stream,
};
use hayro::hayro_syntax::page::Resources;
use hayro::hayro_syntax::xref::XRef;
use hayro::vello_cpu::peniko::kurbo::Shape;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::ops::Range;

/// An optional-content entry this build cannot evaluate (dangling group, malformed `/VE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unevaluable;

/// The default optional-content configuration of a document.
pub struct Layers<'a> {
    xref: &'a XRef,
    base_on: bool,
    listed: HashSet<Oid>,
    on: HashSet<Oid>,
    off: HashSet<Oid>,
    /// `/AS` View events: the group's own `/Usage /View /ViewState`.
    view: HashMap<Oid, bool>,
}

fn refs(d: &Dict<'_>, key: &str) -> Vec<Oid> {
    d.get::<Array<'_>>(key)
        .map(|a| {
            a.raw_iter()
                .filter_map(|m| m.as_obj_ref())
                .map(Oid::from)
                .collect()
        })
        .unwrap_or_default()
}

impl<'a> Layers<'a> {
    pub fn read(xref: &'a XRef) -> Self {
        let mut l = Self {
            xref,
            base_on: true,
            listed: HashSet::new(),
            on: HashSet::new(),
            off: HashSet::new(),
            view: HashMap::new(),
        };
        let Some(props) = xref
            .get::<Dict<'_>>(xref.root_id())
            .and_then(|c| c.get::<Dict<'_>>("OCProperties"))
        else {
            return l;
        };
        l.listed = refs(&props, "OCGs").into_iter().collect();
        let Some(d) = props.get::<Dict<'_>>("D") else {
            return l;
        };
        l.base_on = d
            .get::<Name<'_>>("BaseState")
            .is_none_or(|n| n.as_str() != "OFF");
        l.on = refs(&d, "ON").into_iter().collect();
        l.off = refs(&d, "OFF").into_iter().collect();
        for a in d
            .get::<Array<'_>>("AS")
            .iter()
            .flat_map(|a| a.iter::<Dict<'_>>())
        {
            let event = a
                .get::<Name<'_>>("Event")
                .is_some_and(|n| n.as_str() == "View");
            let category = a
                .get::<Array<'_>>("Category")
                .is_some_and(|c| c.iter::<Name<'_>>().any(|n| n.as_str() == "View"));
            if !(event && category) {
                continue;
            }
            for id in refs(&a, "OCGs") {
                let state = xref
                    .get::<Dict<'_>>(id)
                    .and_then(|g| g.get::<Dict<'_>>("Usage"))
                    .and_then(|u| u.get::<Dict<'_>>("View"))
                    .and_then(|v| v.get::<Name<'_>>("ViewState"));
                if let Some(s) = state {
                    l.view.insert(id, s.as_str() == "ON");
                }
            }
        }
        l
    }

    /// Number of groups the file lists.
    pub fn count(&self) -> u32 {
        self.listed.len() as u32
    }

    fn group_on(&self, id: Oid, g: &Dict<'_>) -> bool {
        let sees = |n: &Name<'_>| matches!(n.as_str(), "View" | "All");
        let view_intent = match g.get::<Object<'_>>("Intent") {
            Some(Object::Name(n)) => sees(&n),
            Some(Object::Array(a)) => a.iter::<Name<'_>>().any(|n| sees(&n)),
            _ => true,
        };
        if !view_intent {
            return true;
        }
        if let Some(v) = self.view.get(&id) {
            return *v;
        }
        if self.off.contains(&id) {
            false
        } else {
            self.on.contains(&id) || self.base_on || !self.listed.contains(&id)
        }
    }

    fn group(&self, id: Oid) -> Result<bool, Unevaluable> {
        let g = self.xref.get::<Dict<'_>>(id).ok_or(Unevaluable)?;
        Ok(self.group_on(id, &g))
    }

    /// PDF 8.11.2.2: a visibility expression `[/And|/Or|/Not operand ...]`.
    fn expression(&self, a: &Array<'_>, depth: u8) -> Result<bool, Unevaluable> {
        if depth > 8 {
            return Err(Unevaluable);
        }
        let mut it = a.raw_iter();
        let Some(MaybeRef::NotRef(Object::Name(op))) = it.next() else {
            return Err(Unevaluable);
        };
        let mut vals = Vec::new();
        for m in it {
            vals.push(match m {
                MaybeRef::Ref(r) => {
                    let id = Oid::from(r);
                    match self.xref.get::<Array<'_>>(id) {
                        Some(nested) => self.expression(&nested, depth + 1)?,
                        None => self.group(id)?,
                    }
                }
                MaybeRef::NotRef(Object::Array(nested)) => self.expression(&nested, depth + 1)?,
                MaybeRef::NotRef(_) => return Err(Unevaluable),
            });
        }
        match (op.as_str(), vals.as_slice()) {
            ("Not", [v]) => Ok(!v),
            ("And", v) if !v.is_empty() => Ok(v.iter().all(|x| *x)),
            ("Or", v) if !v.is_empty() => Ok(v.iter().any(|x| *x)),
            _ => Err(Unevaluable),
        }
    }

    fn membership(&self, d: &Dict<'_>) -> Result<bool, Unevaluable> {
        if let Some(ve) = d.get::<Array<'_>>("VE") {
            return self.expression(&ve, 0);
        }
        let mut ids = refs(d, "OCGs");
        if let (true, Some(r)) = (ids.is_empty(), d.get_ref("OCGs")) {
            ids.push(r.into());
        }
        let states = ids
            .into_iter()
            .map(|id| self.group(id))
            .collect::<Result<Vec<_>, _>>()?;
        if states.is_empty() {
            return Ok(true);
        }
        let policy = d.get::<Name<'_>>("P");
        Ok(match policy.as_ref().map(|n| n.as_str()) {
            Some("AllOn") => states.iter().all(|s| *s),
            Some("AnyOff") => states.iter().any(|s| !s),
            Some("AllOff") => states.iter().all(|s| !s),
            _ => states.iter().any(|s| *s),
        })
    }

    /// Whether the content tagged with `oc` (an OCG or OCMD; `id` is its object when indirect)
    /// is shown by default.
    pub fn shown(&self, oc: &Dict<'_>, id: Option<Oid>) -> Result<bool, Unevaluable> {
        if oc
            .get::<Name<'_>>("Type")
            .is_some_and(|t| t.as_str() == "OCMD")
        {
            return self.membership(oc);
        }
        id.map_or(Ok(true), |id| Ok(self.group_on(id, oc)))
    }

    /// Whether `holder` (an XObject or annotation dictionary) carries an `/OC` that hides it.
    pub fn hides(&self, holder: &Dict<'_>) -> Result<bool, Unevaluable> {
        if !holder.contains_key("OC") {
            return Ok(false);
        }
        let oc = holder.get::<Dict<'_>>("OC").ok_or(Unevaluable)?;
        Ok(!self.shown(&oc, holder.get_ref("OC").map(Oid::from))?)
    }
}

/// Whether the catalog has `/OCProperties`.
pub fn present(doc: &Document) -> bool {
    let x = doc.pdf().xref();
    x.get::<Dict<'_>>(x.root_id())
        .is_some_and(|c| c.contains_key("OCProperties"))
}

// ---- what the model says about each operator ---------------------------------------------------

type Loc = (ObjectId, u32, u32);

/// Every occurrence of one operator, across pages and Form instances.
struct Site {
    /// All occurrences are outside the CropBox or their clip.
    out: bool,
    /// Every glyph drawn here is invisible text (render mode 3 or 7).
    invisible: bool,
    /// Advance in em per code index of a show operator.
    adv: BTreeMap<u32, f64>,
}

type Bounds = [f64; 4];

fn gone(b: Bounds, view: Bounds) -> bool {
    view[0] > view[2]
        || view[1] > view[3]
        || b[2] < view[0]
        || b[0] > view[2]
        || b[3] < view[1]
        || b[1] > view[3]
}

/// Sites of every page, and whether any page paints through a pattern cell or Type3 glyph
/// procedure (not descended by the model).
fn sites(doc: &Document) -> Result<(HashMap<Loc, Site>, bool), PdfError> {
    let mut out: HashMap<Loc, Site> = HashMap::new();
    let mut blind = false;
    for i in 0..doc.page_count() {
        let m = doc.page_model(i)?;
        blind |= m.diagnostics.iter().any(|d| {
            matches!(
                d.kind,
                DiagnosticKind::PatternNotDescended | DiagnosticKind::Type3NotDescended(_)
            )
        });
        let crop = m.geometry.crop_box;
        for it in &m.items {
            let (b, code) = match &it.kind {
                ItemKind::Glyph(g) => {
                    let r = g.quad_user.bounds();
                    (
                        [r.x, r.y, r.x + r.width, r.y + r.height],
                        Some((
                            g.op_glyph_index,
                            g.advance_em,
                            matches!(g.render_mode, 3 | 7),
                        )),
                    )
                }
                ItemKind::Image(im) => {
                    let pts = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
                        .map(|(x, y)| im.ctm.apply(Point { x, y }));
                    let xs = pts.map(|p| p.x);
                    let ys = pts.map(|p| p.y);
                    let (lo, hi) = (f64::min, f64::max);
                    (
                        [
                            xs.into_iter().fold(f64::INFINITY, lo),
                            ys.into_iter().fold(f64::INFINITY, lo),
                            xs.into_iter().fold(f64::NEG_INFINITY, hi),
                            ys.into_iter().fold(f64::NEG_INFINITY, hi),
                        ],
                        None,
                    )
                }
                ItemKind::Path(p) | ItemKind::Shading(p) => {
                    if p.clip_only {
                        continue;
                    }
                    let r = p.geometry.bounding_box();
                    // Half the line width, times the miter limit, is the farthest a stroke reaches.
                    let pad = p.stroke.as_ref().map_or(0.0, |s| {
                        f64::from(s.line_width) * p.stroke_scale * f64::from(s.miter_limit.max(1.0))
                    });
                    ([r.x0 - pad, r.y0 - pad, r.x1 + pad, r.y1 + pad], None)
                }
            };
            let site = out
                .entry((it.loc.stream, it.loc.start, it.loc.end))
                .or_insert_with(|| Site {
                    out: true,
                    invisible: true,
                    adv: BTreeMap::new(),
                });
            site.out &= gone(b, crop) || it.clip.is_some_and(|c| gone(b, c));
            if let Some((index, adv, invisible)) = code {
                site.adv.insert(index, adv);
                site.invisible &= invisible;
            }
        }
    }
    Ok((out, blind))
}

// ---- the stream walk ---------------------------------------------------------------------------

/// Thousandths of an em, never in exponent form.
fn num(v: f64) -> String {
    let s = format!("{v:.5}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

/// A show operator without its glyphs: one `TJ` displacement equal to what they advanced, so
/// later text of the same text object stays where it was.
fn advance_only(ins: &Instruction<'_, '_>, adv: f64) -> Vec<u8> {
    let operands: Vec<&Object<'_>> = ins.operands().collect();
    let mut shift = -1000.0 * adv;
    let mut pre = String::new();
    match (&**ins.operator, &operands[..]) {
        (b"TJ", [Object::Array(a)]) => {
            shift += a
                .iter::<Object<'_>>()
                .filter_map(|o| {
                    if let Object::Number(n) = o {
                        Some(n.as_f64())
                    } else {
                        None
                    }
                })
                .sum::<f64>();
        }
        (b"'", _) => pre.push_str("T*\n"),
        (b"\"", [Object::Number(aw), Object::Number(ac), _]) => {
            let _ = write!(
                pre,
                "{} Tw\n{} Tc\nT*\n",
                num(aw.as_f64()),
                num(ac.as_f64())
            );
        }
        _ => {}
    }
    format!("{pre}[{}] TJ", num(shift)).into_bytes()
}

/// The empty Form that takes the place of an XObject nothing draws any more.
fn stub() -> NewStream {
    let name = |n: &[u8]| Val::Name(n.to_vec());
    NewStream {
        dict: vec![
            ("Type".into(), name(b"XObject")),
            ("Subtype".into(), name(b"Form")),
            ("BBox".into(), Val::Arr(vec![Val::Int(0); 4])),
        ],
        data: Vec::new(),
    }
}

/// What the stage does to a document, decided up front; [`Scan::apply`] performs it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Scan {
    spans: Spans,
    stubs: BTreeSet<ObjectId>,
    /// Layers of `/OCProperties` resolved to their default state.
    pub layers: u32,
    /// Operators removed because optional content hides them.
    pub hidden_ops: u32,
    /// Operators removed because they lie outside the page or their clip.
    pub off_page_ops: u32,
    /// Glyphs of invisible text (render mode 3 or 7) removed under `invisibleText: remove`.
    pub invisible_glyphs: u32,
    /// `ActualText`, `Alt` and `E` entries of marked-content property lists (nested ones too);
    /// the writer never copies them.
    pub accessibility: u32,
    /// Refusals this input raises.
    pub blockers: Vec<Blocker>,
}

impl Scan {
    /// Whether [`Scan::apply`] rewrites no content (it still resolves the layers).
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty() && self.stubs.is_empty()
    }

    pub fn apply(&self, w: &mut Writer<'_>) {
        w.resolve_layers();
        for (stream, subs) in &self.spans {
            for (range, ops) in subs {
                w.replace_span(*stream, range.clone(), ops);
            }
        }
        for id in &self.stubs {
            w.replace_stream(*id, stub());
        }
    }
}

struct Walk<'a> {
    doc: &'a Document,
    remove_invisible: bool,
    layers: Layers<'a>,
    sites: HashMap<Loc, Site>,
    budget: Budget,
    done: HashSet<ObjectId>,
    out: Scan,
    /// `Do`s removed / kept, by XObject.
    removed: HashMap<ObjectId, u32>,
    kept: HashSet<ObjectId>,
    page: u32,
    unevaluable: BTreeSet<u32>,
}

impl<'a> Walk<'a> {
    fn put(&mut self, stream: ObjectId, span: Range<usize>, ops: Vec<u8>, hidden: bool) {
        self.out.spans.entry(stream).or_default().push((span, ops));
        if hidden {
            self.out.hidden_ops += 1;
        } else {
            self.out.off_page_ops += 1;
        }
    }

    /// `r`, or `fallback` (and a refusal for this page) when it cannot be evaluated.
    fn judge(&mut self, r: Result<bool, Unevaluable>, fallback: bool) -> bool {
        r.unwrap_or_else(|_| {
            self.unevaluable.insert(self.page);
            fallback
        })
    }

    /// `Some(shown)` for a marked-content sequence that optional content controls.
    fn oc_of(&mut self, ins: &Instruction<'_, '_>, res: &Resources<'a>) -> Option<bool> {
        let mut ops = ins.operands();
        let (Some(Object::Name(tag)), Some(props)) = (ops.next(), ops.next()) else {
            return None;
        };
        if tag.as_str() != "OC" {
            return None;
        }
        let shown = match props {
            Object::Name(n) => match res.properties.get::<Dict<'_>>(n) {
                Some(d) => self
                    .layers
                    .shown(&d, res.properties.get_ref(n).map(Oid::from)),
                None => Err(Unevaluable),
            },
            Object::Dict(d) => {
                let oc = d.get::<Dict<'_>>("OC")?;
                self.layers.shown(&oc, d.get_ref("OC").map(Oid::from))
            }
            _ => return None,
        };
        Some(self.judge(shown, true))
    }

    fn run(
        &mut self,
        stream: ObjectId,
        data: &[u8],
        res: &Resources<'a>,
        mc: &mut Vec<Option<bool>>,
        depth: usize,
    ) -> Result<(), PdfError> {
        let mut it = UntypedIter::new(data);
        while let Some((ins, span)) = it.next_with_span() {
            self.budget.charge(1)?;
            let op: &[u8] = ins.operator;
            let hidden = mc.contains(&Some(false));
            let site = self
                .sites
                .get(&(stream, span.start as u32, span.end as u32));
            let (off, adv) = site.map_or((false, 0.0), |s| (s.out, s.adv.values().sum::<f64>()));
            let ghost = self.remove_invisible && site.is_some_and(|s| s.invisible);
            let ghosts = site.map_or(0, |s| s.adv.len() as u32);
            let gone = hidden || off;
            match op {
                b"BDC" => {
                    self.out.accessibility += text_semantics::accessibility_entries(&ins, res);
                    let oc = self.oc_of(&ins, res);
                    mc.push(oc);
                    if oc.is_some() {
                        self.out
                            .spans
                            .entry(stream)
                            .or_default()
                            .push((span, Vec::new()));
                    }
                }
                b"BMC" => mc.push(None),
                b"EMC" => {
                    if let Some(Some(_)) = mc.pop() {
                        self.out
                            .spans
                            .entry(stream)
                            .or_default()
                            .push((span, Vec::new()));
                    }
                }
                b"Tj" | b"TJ" | b"'" | b"\"" if gone || ghost => {
                    let ops = advance_only(&ins, adv);
                    if gone {
                        self.put(stream, span, ops, hidden);
                    } else {
                        self.out.spans.entry(stream).or_default().push((span, ops));
                        self.out.invisible_glyphs += ghosts;
                    }
                }
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" if gone => {
                    self.put(stream, span, b"n".to_vec(), hidden);
                }
                b"sh" | b"BI" if gone => self.put(stream, span, Vec::new(), hidden),
                b"Do" => {
                    let Some(Object::Name(name)) = ins.operands().next() else {
                        continue;
                    };
                    let Some(x) = res.get_x_object(name) else {
                        continue;
                    };
                    let id = object_id(x.obj_id().into());
                    let own = self.layers.hides(x.dict());
                    let hidden = hidden || self.judge(own, false);
                    let image = match x
                        .dict()
                        .get::<Name<'_>>("Subtype")
                        .as_ref()
                        .map(|n| n.as_str())
                    {
                        Some("Image") => true,
                        Some("Form") => false,
                        _ => continue,
                    };
                    if hidden || (image && off) {
                        *self.removed.entry(id).or_default() += 1;
                        self.put(stream, span, Vec::new(), hidden);
                    } else {
                        self.kept.insert(id);
                        if !image {
                            self.form(x, res, depth + 1)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn form(
        &mut self,
        s: Stream<'a>,
        parent: &Resources<'a>,
        depth: usize,
    ) -> Result<(), PdfError> {
        let id = object_id(s.obj_id().into());
        if !self.done.insert(id) {
            return Ok(());
        }
        limits::check(
            "maxNesting",
            depth as u64,
            u64::from(self.doc.limits().max_nesting) + 1,
        )?;
        let data = limits::decode_stream(&s, id, self.doc.limits(), &mut self.budget)?;
        let res = s
            .dict()
            .get::<Dict<'_>>("Resources")
            .map_or_else(|| parent.clone(), Resources::new);
        self.run(id, &data, &res, &mut Vec::new(), depth)
    }

    fn page(&mut self, index: usize) -> Result<(), PdfError> {
        self.page = index as u32;
        let page = self.doc.page(index)?;
        let res = page.resources().clone();
        let mut mc = Vec::new();
        for c in self.doc.page_contents_with(index, &mut self.budget)? {
            if self.done.insert(c.obj) {
                self.run(c.obj, &c.data, &res, &mut mc, 0)?;
            }
        }
        // Appearance streams are drawn by the interactive stage; their layers resolve too.
        let Some(annots) = page.raw().get::<Array<'_>>("Annots") else {
            return Ok(());
        };
        for a in annots.iter::<Dict<'_>>() {
            let own = self.layers.hides(&a);
            if self.judge(own, false) {
                continue;
            }
            match a
                .get::<Dict<'_>>("AP")
                .and_then(|ap| ap.get::<Object<'_>>("N"))
            {
                Some(Object::Stream(s)) => self.form(s, &res, 1)?,
                Some(Object::Dict(states)) => {
                    for key in states.keys() {
                        if let Some(s) = states.get::<Stream<'_>>(key) {
                            self.form(s, &res, 1)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Decide the whole stage for `doc`. `Err` only for what the reader and the budgets refuse;
/// what this stage cannot evaluate comes back as [`Scan::blockers`].
pub fn scan(doc: &Document) -> Result<Scan, PdfError> {
    scan_with(doc, false)
}

/// [`scan`], and with `remove_invisible` also the text shows whose every glyph is invisible
/// (render mode 3 or 7): replaced like hidden text, advances kept.
pub fn scan_with(doc: &Document, remove_invisible: bool) -> Result<Scan, PdfError> {
    let (sites, blind) = sites(doc)?;
    let mut w = Walk {
        doc,
        remove_invisible,
        layers: Layers::read(doc.pdf().xref()),
        sites,
        budget: Budget::new(doc.limits()),
        done: HashSet::new(),
        out: Scan::default(),
        removed: HashMap::new(),
        kept: HashSet::new(),
        page: 0,
        unevaluable: BTreeSet::new(),
    };
    for i in 0..doc.page_count() {
        w.page(i)?;
    }
    let mut out = w.out;
    out.layers = w.layers.count();
    out.stubs = w
        .removed
        .keys()
        .filter(|id| !w.kept.contains(id))
        .copied()
        .collect();
    let refuse = |feature, pages: Vec<u32>| Blocker {
        code: ErrorCode::OptionalContentUnsupported,
        feature,
        pages,
        fields: Vec::new(),
        ids: Vec::new(),
    };
    if !w.unevaluable.is_empty() {
        out.blockers.push(refuse(
            "flattenOptionalContent",
            w.unevaluable.into_iter().collect(),
        ));
    }
    if blind && !out.stubs.is_empty() {
        out.blockers.push(refuse("ocgPatternResources", Vec::new()));
    }
    Ok(out)
}
