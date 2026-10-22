//! Annotations and form widgets (spec §8.2, HDDN-107): each visible one is drawn once, as a Form
//! XObject placed by the PDF 12.5.5 mapping, at the end of its page's content, so redaction
//! (which already sees annotation appearances in the semantic model) treats the drawn text,
//! images and vectors as page content. Everything else about annotations and the form is dropped
//! by the allowlist writer: field values, `/AcroForm`, JavaScript, `/NeedAppearances`, XFA,
//! signatures, popups, links, alternate (`/R`, `/D`) appearances and unselected states.
//!
//! Annotations obey `policy.annotations`; widgets are always flattened (the user filled the
//! form to be read). Refusals, decided before any byte is written: `FORM_APPEARANCE_MISSING
//! {fields}` (a visible field holds a value no usable `/AP /N` shows, or `/NeedAppearances` makes
//! the stored appearance of a valued field stale) and `XFA_UNSUPPORTED` (XFA with no widget
//! appearance to keep). Signature values are counted by [`super::document`]; their widgets are
//! flattened like any other.
use super::visibility::Layers;
use crate::export::preflight::Blocker;
use crate::reader::{Document, object_id};
use crate::writer::{Val, Writer};
use foliark_model::{AnnotationPolicy, ErrorCode, ExportPolicy, ObjectId, PdfError};
use hayro::hayro_syntax::object::{Array, Dict, Name, Object, Stream, String as PdfString};
use std::collections::BTreeSet;
use std::fmt::Write as _;

const HIDDEN: u32 = 2;
const NO_VIEW: u32 = 32;
const NO_ROTATE: u32 = 16;

#[derive(Debug, Clone, PartialEq)]
struct Draw {
    page: usize,
    ap: ObjectId,
    /// A of 12.5.5: the transformed `BBox` onto `Rect`; the Form's `Matrix` is applied by `Do`.
    cm: [f64; 6],
    widget: bool,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Scan {
    draws: Vec<Draw>,
    /// Non-widget annotations of all pages (links and popups included).
    pub annotations: u32,
    pub links: u32,
    pub widgets: u32,
    /// Annotations and widgets shown by their flags that an OFF `/OC` hides: not drawn.
    pub oc_hidden: u32,
    /// Visible non-widget annotations dropped without drawing: policy `remove`, or no appearance.
    pub removed_visible: u32,
    /// Refusals this input raises (empty = the interactive stage can run).
    pub blockers: Vec<Blocker>,
}

impl Scan {
    /// Whether [`Scan::apply`] draws nothing.
    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }

    /// (annotations, widgets) that [`Scan::apply`] draws.
    pub fn flattened(&self) -> (u32, u32) {
        let widgets = self.draws.iter().filter(|d| d.widget).count() as u32;
        (self.draws.len() as u32 - widgets, widgets)
    }

    /// Pages that get a flattened appearance, by kind (non-widget, widget).
    pub fn pages(&self) -> (Vec<u32>, Vec<u32>) {
        let of = |widget| {
            let set: BTreeSet<u32> = self
                .draws
                .iter()
                .filter(|d| d.widget == widget)
                .map(|d| d.page as u32)
                .collect();
            set.into_iter().collect()
        };
        (of(false), of(true))
    }

    /// Draw every flattened appearance on top of its page. Call before the redaction fills.
    pub fn apply(&self, w: &mut Writer<'_>) -> Result<(), PdfError> {
        let mut ops = std::collections::BTreeMap::<usize, String>::new();
        for (n, d) in self.draws.iter().enumerate() {
            let name = format!("FoliarkFlat{n}");
            w.add_page_xobject(d.page, &name, Val::Src(d.ap))?;
            let c = d.cm;
            let _ = writeln!(
                ops.entry(d.page).or_default(),
                "q {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} cm /{name} Do Q",
                c[0],
                c[1],
                c[2],
                c[3],
                c[4],
                c[5]
            );
        }
        for (page, ops) in ops {
            w.append_page_content(page, ops.as_bytes())?;
        }
        Ok(())
    }
}

fn nums(a: Option<Array<'_>>, n: usize) -> Option<Vec<f64>> {
    let v: Vec<f64> = a?.iter::<f64>().collect();
    (v.len() == n && v.iter().all(|x| x.is_finite())).then_some(v)
}

/// The normal appearance stream of `annot`: `/AP /N` itself, or its entry for `/AS`.
fn normal<'a>(annot: &Dict<'a>) -> Option<Stream<'a>> {
    let ap = annot.get::<Dict<'a>>("AP")?;
    if let Some(s) = ap.get::<Stream<'a>>("N") {
        return Some(s);
    }
    let states = ap.get::<Dict<'a>>("N")?;
    let state = annot.get::<Name<'a>>("AS")?;
    states.get::<Stream<'a>>(state.as_str())
}

/// PDF 12.5.5: `Matrix` followed by the scale and translation that fit the transformed `BBox`
/// onto `Rect`. `None` for an appearance or rectangle with no area. `upright` is the page's
/// clockwise `/Rotate` for a `NoRotate` annotation: its appearance stays upright on the displayed
/// page with the rectangle's upper-left corner (in the unrotated page) fixed (PDF 12.5.3).
fn placement(ap: &Stream<'_>, rect: &[f64], upright: u16) -> Option<[f64; 6]> {
    let d = ap.dict();
    let b = nums(d.get::<Array<'_>>("BBox"), 4)?;
    let m = nums(d.get::<Array<'_>>("Matrix"), 6).unwrap_or(vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for (x, y) in [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])] {
        let p = [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
        for i in 0..2 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let (rx0, rx1) = (rect[0].min(rect[2]), rect[0].max(rect[2]));
    let (ry0, ry1) = (rect[1].min(rect[3]), rect[1].max(rect[3]));
    let (tw, th) = (hi[0] - lo[0], hi[1] - lo[1]);
    if tw <= 0.0 || th <= 0.0 || rx1 <= rx0 || ry1 <= ry0 {
        return None;
    }
    let (sx, sy) = ((rx1 - rx0) / tw, (ry1 - ry0) / th);
    let (ex, ey) = (rx0 - lo[0] * sx, ry0 - lo[1] * sy);
    // `Do` applies the Form's own `/Matrix` (step 3's `Matrix x A`), so only A is emitted.
    let (ax, ay) = (rx0, ry1);
    Some(match upright {
        90 => [0.0, sx, -sy, 0.0, ax + hi[1] * sy, ay - lo[0] * sx],
        180 => [-sx, 0.0, 0.0, -sy, ax + lo[0] * sx, ay + hi[1] * sy],
        270 => [0.0, -sx, sy, 0.0, ax - hi[1] * sy, ay + lo[0] * sx],
        _ => [sx, 0.0, 0.0, sy, ex, ey],
    })
}

/// An inheritable field attribute: the widget's own, else the nearest ancestor's.
fn inherited<'a>(widget: &Dict<'a>, key: &str) -> Option<Object<'a>> {
    let mut d = widget.clone();
    for _ in 0..32 {
        if let Some(o) = d.get::<Object<'a>>(key) {
            return Some(o);
        }
        d = d.get::<Dict<'a>>("Parent")?;
    }
    None
}

fn text(s: &PdfString<'_>) -> String {
    let b = s.as_bytes();
    match b.strip_prefix(&[0xFE, 0xFF]) {
        Some(u) => String::from_utf16_lossy(
            &u.as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect::<Vec<_>>(),
        ),
        None => String::from_utf8_lossy(b).into_owned(),
    }
}

/// Fully qualified field name: the `/T` chain from the root down.
fn field_name(widget: &Dict<'_>) -> String {
    let mut parts = Vec::new();
    let mut d = Some(widget.clone());
    for _ in 0..32 {
        let Some(cur) = d else { break };
        if let Some(t) = cur.get::<PdfString<'_>>("T") {
            parts.push(text(&t));
        }
        d = cur.get::<Dict<'_>>("Parent");
    }
    parts.reverse();
    if parts.is_empty() {
        "(unnamed)".into()
    } else {
        parts.join(".")
    }
}

fn has_value(o: &Object<'_>) -> bool {
    match o {
        Object::String(s) => !s.as_bytes().is_empty(),
        Object::Name(n) => n.as_str() != "Off",
        Object::Array(a) => a.raw_iter().next().is_some(),
        Object::Number(_) | Object::Stream(_) => true,
        _ => false,
    }
}

/// Classify every annotation of `doc` under `policy`.
pub fn scan(doc: &Document, policy: &ExportPolicy) -> Scan {
    let mut out = Scan::default();
    let root = doc.pdf().xref().root_id();
    let acro = doc
        .pdf()
        .xref()
        .get::<Dict<'_>>(root)
        .and_then(|c| c.get::<Dict<'_>>("AcroForm"));
    let stale = acro
        .as_ref()
        .and_then(|a| a.get::<bool>("NeedAppearances"))
        .unwrap_or(false);
    let xfa = acro.as_ref().is_some_and(|a| a.contains_key("XFA"));
    let layers = Layers::read(doc.pdf().xref());
    let mut missing: BTreeSet<String> = BTreeSet::new();
    let mut missing_pages: BTreeSet<u32> = BTreeSet::new();
    for (i, page) in doc.pdf().pages().iter().enumerate() {
        let Some(list) = page.raw().get::<Array<'_>>("Annots") else {
            continue;
        };
        let rotation = doc.page_geometry(i).map_or(0, |g| g.rotation);
        for a in list.iter::<Dict<'_>>() {
            let kind = a.get::<Name<'_>>("Subtype");
            let kind = kind.as_ref().map(|n| n.as_str());
            let widget = kind == Some("Widget");
            let flagged = a.get::<u32>("F").unwrap_or(0) & (HIDDEN | NO_VIEW) == 0;
            // An `/OC` this build cannot evaluate refuses in `visibility::scan`.
            let off = layers.hides(&a).unwrap_or(false);
            out.oc_hidden += u32::from(flagged && off);
            let shown = flagged && !off;
            let appearance = normal(&a);
            let upright = if a.get::<u32>("F").unwrap_or(0) & NO_ROTATE != 0 {
                rotation
            } else {
                0
            };
            let place = appearance.as_ref().and_then(|ap| {
                let rect = nums(a.get::<Array<'_>>("Rect"), 4)?;
                Some((
                    object_id(ap.obj_id().into()),
                    placement(ap, &rect, upright)?,
                ))
            });
            if widget {
                out.widgets += 1;
                let sig = inherited(&a, "FT")
                    .is_some_and(|o| matches!(o, Object::Name(n) if n.as_str() == "Sig"));
                let valued = !sig && inherited(&a, "V").is_some_and(|v| has_value(&v));
                if shown && valued && (place.is_none() || stale) {
                    missing.insert(field_name(&a));
                    missing_pages.insert(i as u32);
                }
                if let (true, Some((ap, cm))) = (shown, place) {
                    out.draws.push(Draw {
                        page: i,
                        ap,
                        cm,
                        widget: true,
                    });
                }
                continue;
            }
            out.annotations += 1;
            match kind {
                Some("Link") => out.links += 1,
                Some("Popup") => {}
                _ => {
                    // Shown but not drawn (policy `remove`, or no appearance to draw): removed.
                    match (shown, place) {
                        (true, Some((ap, cm)))
                            if policy.annotations == AnnotationPolicy::Flatten =>
                        {
                            out.draws.push(Draw {
                                page: i,
                                ap,
                                cm,
                                widget: false,
                            });
                        }
                        (true, _) => out.removed_visible += 1,
                        _ => {}
                    }
                }
            }
        }
    }
    if !missing.is_empty() {
        out.blockers.push(Blocker {
            code: ErrorCode::FormAppearanceMissing,
            feature: "forms",
            pages: missing_pages.into_iter().collect(),
            fields: missing.into_iter().collect(),
            ids: Vec::new(),
        });
    }
    if xfa && out.flattened().1 == 0 {
        out.blockers.push(Blocker {
            code: ErrorCode::XfaUnsupported,
            feature: "xfa",
            pages: Vec::new(),
            fields: Vec::new(),
            ids: Vec::new(),
        });
    }
    out
}
