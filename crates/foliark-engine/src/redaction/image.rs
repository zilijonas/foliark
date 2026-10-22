//! Image erasure (spec §5.2), executed through the writer.
//!
//! For every planned image occurrence the source is decoded with hayro (strict for plain
//! filters, capped by the resource limits), every sample whose pixel square meets a region in
//! display space is set to black, and the result is written as a fresh 8-bit DeviceRGB or
//! DeviceGray Flate stream. The compressed source bytes never reach the output: an image drawn
//! once is replaced in place, an image drawn several times gets a clone for each erased
//! occurrence (the `Do` is rewritten to the clone) and the original stays reachable only
//! through the occurrences that were not erased. An inline image becomes an XObject drawn by
//! `Do` in place of `BI ... EI`.
//!
//! Pixel-square test: the display-space parallelogram of a sample and the region rectangle
//! are tested with separating axes on closed sets (touching counts), so an edge sample is
//! erased rather than kept. Page `/Rotate`, `/CropBox` and any CTM (rotation, scale, skew,
//! flip) are inside the `user_to_display` and `ctm` matrices.
//!
//! Limits, all typed refusals, none silent:
//! - A clone or an inline conversion needs a resource entry, which only a page has here: an
//!   occurrence drawn through a Form refuses with `INVALID_ARGUMENT`,
//!   `details.feature = "redactionFormImageClone"` (NEW-38 clones Forms). So does a content
//!   stream shared by several pages (`"redactionSharedContent"`).
//! - Decoded size beyond the limits is `LIMIT_EXCEEDED`; an undecodable image, or one whose
//!   decoded size differs from its dictionary, is `DECODE_FAILED` naming the object.
//! - An inline image whose color space is a named resource is refused (`"redactionInlineColorSpace"`).
//!
//! Disclosed, not refused: color spaces other than DeviceRGB/DeviceGray are converted to
//! rendered RGB or gray ([`ImageReport::color_converted`]); `Decode` arrays and bit depths are
//! applied and dropped; `Matte` is applied. Nothing of this is reversible, which is the point.
//!
//! On `Err` the writer is untouched: everything is decoded and encoded before the first call.
use super::masks;
use super::plan::{PlannedImage, RedactionPlan};
use crate::Document;
use crate::interpret::{ImageItem, ItemKind};
use crate::limits::{self, Budget};
use crate::writer::{NewStream, Val, Writer};
use foliark_model::{ErrorCode, ErrorDetails, Limits, ObjectId, PdfError};
use hayro::hayro_interpret::{ImageData, RasterImage};
use hayro::hayro_syntax::Filter;
use hayro::hayro_syntax::content::UntypedIter;
use hayro::hayro_syntax::object::{Dict, Name, Object, ObjectIdentifier, Stream};
use hayro::vello_cpu::peniko::kurbo::{Affine, Point};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageReport {
    /// Occurrences erased (one per planned image).
    pub images_erased: u32,
    /// Of those, the ones that got a new object next to a surviving original.
    pub images_cloned: u32,
    /// Inline images turned into XObjects.
    pub inline_converted: u32,
    /// Erased images whose source color space was not already DeviceRGB or DeviceGray.
    pub color_converted: u32,
}

fn refuse(feature: &str, page: u32) -> PdfError {
    PdfError::with(
        ErrorCode::InvalidArgument,
        ErrorDetails {
            feature: Some(feature.into()),
            pages: Some(vec![page]),
            ..ErrorDetails::default()
        },
    )
}

fn at(id: ObjectId) -> ObjectIdentifier {
    ObjectIdentifier::new(id.num as i32, i32::from(id.generation))
}

/// One grid of samples (an image plane, a soft mask) and where it sits.
pub(super) struct Plane {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

impl Plane {
    pub fn check(self, channels: usize, obj: ObjectId) -> Result<Self, PdfError> {
        let want = (self.w as usize).checked_mul(self.h as usize * channels);
        if self.w == 0 || self.h == 0 || want != Some(self.data.len()) {
            return Err(PdfError::at(ErrorCode::DecodeFailed, obj));
        }
        Ok(self)
    }
}

/// Every sample of a `w` x `h` grid filling the unit square mapped by `m` (row 0 at the top,
/// y = 1) whose pixel square meets one of `rects` (`[x0, y0, x1, y1]`), as `visit(index)`.
pub(super) fn erase_grid(
    w: u32,
    h: u32,
    m: Affine,
    rects: &[[f64; 4]],
    mut visit: impl FnMut(usize),
) -> Result<(), PdfError> {
    if m.determinant().abs() < 1e-12 || !m.determinant().is_finite() {
        return Err(PdfError::new(ErrorCode::Internal));
    }
    let inv = m.inverse();
    let (wf, hf) = (f64::from(w), f64::from(h));
    for r in rects {
        // The region in unit space bounds the candidates; the exact test decides.
        let us = [(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])]
            .map(|(x, y)| inv * Point::new(x, y));
        let fold = |f: fn(&Point) -> f64| {
            us.iter()
                .map(f)
                .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)))
        };
        let (u0, u1) = fold(|p| p.x);
        let (v0, v1) = fold(|p| p.y);
        let span = |lo: f64, hi: f64, n: f64| {
            let (a, b) = ((lo * n).floor() - 1.0, (hi * n).ceil() + 1.0);
            (a.clamp(0.0, n) as usize, b.clamp(0.0, n) as usize)
        };
        let (i0, i1) = span(u0, u1, wf);
        // Row j covers v in [1 - (j + 1) / h, 1 - j / h].
        let (j0, j1) = span(1.0 - v1, 1.0 - v0, hf);
        for j in j0..j1.min(h as usize) {
            for i in i0..i1.min(w as usize) {
                let (ua, ub) = (i as f64 / wf, (i + 1) as f64 / wf);
                let (va, vb) = (1.0 - (j + 1) as f64 / hf, 1.0 - j as f64 / hf);
                let c = [(ua, va), (ub, va), (ub, vb), (ua, vb)].map(|(u, v)| m * Point::new(u, v));
                if touches(&c, r) {
                    visit(j * w as usize + i);
                }
            }
        }
    }
    Ok(())
}

/// Closed-set intersection of a convex quad and a rectangle (separating axes).
fn touches(quad: &[Point; 4], r: &[f64; 4]) -> bool {
    const EPS: f64 = 1e-9;
    let rect = [(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])];
    let apart = |axis: (f64, f64)| {
        let proj = |x: f64, y: f64| x * axis.0 + y * axis.1;
        let (qlo, qhi) = quad
            .iter()
            .map(|p| proj(p.x, p.y))
            .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        let (rlo, rhi) = rect
            .iter()
            .map(|&(x, y)| proj(x, y))
            .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        qhi < rlo - EPS || qlo > rhi + EPS
    };
    let norm = |a: Point, b: Point| {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len = dx.hypot(dy).max(f64::MIN_POSITIVE);
        (-dy / len, dx / len)
    };
    !(apart((1.0, 0.0))
        || apart((0.0, 1.0))
        || apart(norm(quad[0], quad[1]))
        || apart(norm(quad[1], quad[2])))
}

/// An image decoded to what hayro draws: 8-bit gray or RGB samples and, for a masked image,
/// its alpha plane (a grid of its own).
struct Decoded {
    pub color: Plane,
    pub rgb: bool,
    pub interpolate: bool,
    pub alpha: Option<Plane>,
}

fn is_codec(s: &Stream<'_>) -> bool {
    s.filters().iter().any(|f| {
        matches!(
            f,
            Filter::DctDecode | Filter::JpxDecode | Filter::CcittFaxDecode | Filter::Jbig2Decode
        )
    })
}

/// The decoded size, checked against the limits before anything is allocated.
fn budget_for(w: u32, h: u32, limits: &Limits, budget: &mut Budget) -> Result<(), PdfError> {
    let pixels = limits::image_pixels(limits, w, h)?;
    // Colour (3 bytes) and one alpha plane.
    limits::check(
        "maxStreamDecodedBytes",
        pixels.saturating_mul(4),
        limits.max_stream_decoded_bytes.into(),
    )?;
    budget.charge_pixels(pixels)?;
    budget.charge_bytes((pixels.saturating_mul(4)).min(usize::MAX as u64) as usize)
}

/// Limits and, for plain filters, a strict decode (hayro pads a truncated or corrupt plain
/// stream with zeros; this refuses it) before the image is decoded.
pub(super) fn precheck(
    stream: &Stream<'_>,
    item: &ImageItem,
    id: ObjectId,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<(), PdfError> {
    budget_for(item.width, item.height, limits, budget)?;
    if !is_codec(stream) {
        limits::decode_stream(stream, id, limits, budget)?;
    }
    Ok(())
}

fn decode(
    stream: &Stream<'_>,
    item: &ImageItem,
    id: ObjectId,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Decoded, PdfError> {
    let failed = || PdfError::at(ErrorCode::DecodeFailed, id);
    precheck(stream, item, id, limits, budget)?;
    let raster = RasterImage::from_stream(stream).ok_or_else(failed)?;
    let mut out = None;
    raster.with_rgba(
        |image, alpha| {
            let (rgb, interpolate, scale, plane) = match image {
                ImageData::Rgb(d) => (
                    true,
                    d.interpolate,
                    d.scale_factors,
                    Plane {
                        w: d.width,
                        h: d.height,
                        data: d.data,
                    },
                ),
                ImageData::Luma(d) => (
                    false,
                    d.interpolate,
                    d.scale_factors,
                    Plane {
                        w: d.width,
                        h: d.height,
                        data: d.data,
                    },
                ),
            };
            let alpha = alpha.map(|a| Plane {
                w: a.width,
                h: a.height,
                data: a.data,
            });
            out = Some((rgb, interpolate, scale, plane, alpha));
        },
        None,
    );
    let (rgb, interpolate, scale, color, alpha) = out.ok_or_else(failed)?;
    // The occurrence maps the whole image onto its unit square: a decode at another size
    // (damaged dimensions, a lower-resolution codec level) cannot be erased faithfully.
    if scale != (1.0, 1.0) || (color.w, color.h) != (item.width, item.height) {
        return Err(failed());
    }
    let color = color.check(if rgb { 3 } else { 1 }, id)?;
    let alpha = alpha.map(|a| a.check(1, id)).transpose()?;
    // A mask the dictionary promises but hayro could not produce would be dropped silently.
    let d = stream.dict();
    let promised = d.contains_key("SMask")
        || d.contains_key("Mask")
        || d.get::<u8>("SMaskInData").is_some_and(|v| v > 0);
    if promised && alpha.is_none() {
        return Err(failed());
    }
    Ok(Decoded {
        color,
        rgb,
        interpolate,
        alpha,
    })
}

/// The source color space needed converting.
fn converted(dict: &Dict<'_>, rgb: bool) -> bool {
    let want: &[&str] = if rgb {
        &["DeviceRGB", "RGB"]
    } else {
        &["DeviceGray", "G"]
    };
    let name = ["ColorSpace", "CS"]
        .iter()
        .find_map(|k| dict.get::<Name<'_>>(*k))
        .map(|n| n.as_str().to_owned());
    !name.is_some_and(|n| want.contains(&n.as_str()))
}

/// What replaces one occurrence: the image dictionary entries and plain data, plus a soft
/// mask when the image had alpha.
pub(super) struct Encoded {
    pub dict: Vec<(String, Val)>,
    pub data: Vec<u8>,
    pub smask: Option<Box<Encoded>>,
}

pub(super) fn image_dict(w: u32, h: u32, cs: &str, interpolate: bool) -> Vec<(String, Val)> {
    let name = |n: &str| Val::Name(n.as_bytes().to_vec());
    let mut d = vec![
        ("Type".into(), name("XObject")),
        ("Subtype".into(), name("Image")),
        ("Width".into(), Val::Int(w as i32)),
        ("Height".into(), Val::Int(h as i32)),
        ("ColorSpace".into(), name(cs)),
        ("BitsPerComponent".into(), Val::Int(8)),
    ];
    if interpolate {
        d.push(("Interpolate".into(), Val::Bool(true)));
    }
    d
}

/// Where an occurrence's replacement goes.
enum Target {
    /// The image object itself: drawn once in the whole document.
    InPlace(ObjectId),
    /// A new object drawn by `/name Do` in place of the instruction at `span`.
    Clone {
        page: u32,
        span: (ObjectId, std::ops::Range<usize>),
    },
}

struct Job {
    target: Target,
    enc: Encoded,
}

fn rects(plan: &PlannedImage, regions: &[super::plan::PlannedRegion]) -> Vec<[f64; 4]> {
    plan.regions
        .iter()
        .map(|&i| {
            let r = regions[i as usize].requested;
            [r.x, r.y, r.x + r.width, r.y + r.height]
        })
        .collect()
}

fn stream_of<'a>(doc: &'a Document, id: ObjectId) -> Result<Stream<'a>, PdfError> {
    doc.reader()
        .pdf()
        .xref()
        .get::<Stream<'a>>(at(id))
        .ok_or(PdfError::at(ErrorCode::MalformedPdf, id))
}

/// The inline image stream of the `BI ... EI` instruction at `span` and its encoding.
fn encode_inline(
    doc: &Document,
    stream: ObjectId,
    span: std::ops::Range<usize>,
    item: &ImageItem,
    m: Affine,
    rects: &[[f64; 4]],
    page: u32,
) -> Result<(Encoded, bool), PdfError> {
    let r = doc.reader();
    let mut budget = Budget::new(r.limits());
    let content = limits::decode_stream(&stream_of(doc, stream)?, stream, r.limits(), &mut budget)?;
    let bad = || PdfError::at(ErrorCode::MalformedPdf, stream);
    let mut it = UntypedIter::new(content.get(span).ok_or_else(bad)?);
    let ins = it.next().ok_or_else(bad)?;
    let image = ins
        .operands()
        .find_map(|o| {
            if let Object::Stream(s) = o {
                Some(s.clone())
            } else {
                None
            }
        })
        .ok_or_else(bad)?;
    let dict = image.dict();
    let named = ["CS", "ColorSpace"]
        .iter()
        .find_map(|k| dict.get::<Name<'_>>(*k));
    if named.is_some_and(|n| {
        !["G", "RGB", "CMYK", "DeviceGray", "DeviceRGB", "DeviceCMYK"].contains(&n.as_str())
    }) {
        return Err(refuse("redactionInlineColorSpace", page));
    }
    encode(&image, item, stream, m, rects, r.limits(), &mut budget)
}

/// The resource name the `Do` instruction at `span` draws.
fn do_name(
    doc: &Document,
    stream: ObjectId,
    span: std::ops::Range<usize>,
) -> Result<String, PdfError> {
    let r = doc.reader();
    let mut budget = Budget::new(r.limits());
    let content = limits::decode_stream(&stream_of(doc, stream)?, stream, r.limits(), &mut budget)?;
    let bad = || PdfError::at(ErrorCode::MalformedPdf, stream);
    let mut it = UntypedIter::new(content.get(span).ok_or_else(bad)?);
    let ins = it.next().ok_or_else(bad)?;
    ins.operands()
        .find_map(|o| {
            if let Object::Name(n) = o {
                Some(n.as_str().to_owned())
            } else {
                None
            }
        })
        .ok_or_else(bad)
}

/// Decode, erase and encode one image stream. The flag says the color space was converted.
fn encode(
    stream: &Stream<'_>,
    item: &ImageItem,
    id: ObjectId,
    m: Affine,
    rects: &[[f64; 4]],
    limits: &Limits,
    budget: &mut Budget,
) -> Result<(Encoded, bool), PdfError> {
    if item.is_stencil {
        return masks::encode_stencil(stream, item, id, m, rects, limits, budget)
            .map(|e| (e, false));
    }
    let mut d = decode(stream, item, id, limits, budget)?;
    let channels = if d.rgb { 3 } else { 1 };
    erase_grid(d.color.w, d.color.h, m, rects, |s| {
        d.color.data[s * channels..(s + 1) * channels].fill(0)
    })?;
    let smask = d
        .alpha
        .take()
        .map(|a| masks::erase_alpha(a, m, rects).map(Box::new))
        .transpose()?;
    let enc = Encoded {
        dict: image_dict(
            d.color.w,
            d.color.h,
            if d.rgb { "DeviceRGB" } else { "DeviceGray" },
            d.interpolate,
        ),
        data: d.color.data,
        smask,
    };
    Ok((enc, converted(stream.dict(), d.rgb)))
}

fn dict_val(entries: Vec<(String, Val)>) -> Val {
    Val::Dict(
        entries
            .into_iter()
            .map(|(k, v)| (k.into_bytes(), v))
            .collect(),
    )
}

/// Add the soft mask (if any) as an object and return the image dictionary entries.
fn with_smask(w: &mut Writer<'_>, enc: Encoded) -> (Vec<(String, Val)>, Vec<u8>) {
    let mut dict = enc.dict;
    if let Some(mask) = enc.smask {
        let (md, mdata) = with_smask(w, *mask);
        dict.push(("SMask".into(), w.add_object(dict_val(md), Some(mdata))));
    }
    (dict, enc.data)
}

/// Erase the planned images of `plan` from `w`'s output. `doc` is the working copy the plan
/// was built from (and `w` writes).
pub fn redact_images(
    doc: &Document,
    plan: &RedactionPlan,
    w: &mut Writer<'_>,
) -> Result<ImageReport, PdfError> {
    let r = doc.reader();
    let mut report = ImageReport::default();
    let mut budget = Budget::new(r.limits());
    let mut jobs = Vec::new();
    let mut models = HashMap::new();
    // Resource entries of images whose occurrence was cloned, and the (page, image) pairs that
    // still have an occurrence drawing the original.
    let mut cloned_names: Vec<(u32, ObjectId, String)> = Vec::new();
    let mut survivors: HashSet<(u32, ObjectId)> = HashSet::new();
    for page in &plan.pages {
        for pi in &page.images {
            let model = match models.entry(page.page_index) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(doc.page_model(page.page_index as usize)?)
                }
            };
            let planned: HashSet<_> = page.images.iter().map(|p| (p.loc, &p.path)).collect();
            for it in &model.items {
                if let ItemKind::Image(ImageItem { obj: Some(o), .. }) = &it.kind
                    && !planned.contains(&(it.loc, &it.path))
                {
                    survivors.insert((page.page_index, *o));
                }
            }
            let item = model
                .items
                .iter()
                .find_map(|it| match &it.kind {
                    ItemKind::Image(i)
                        if it.loc == pi.loc && it.path == pi.path && i.obj == pi.obj =>
                    {
                        Some(i)
                    }
                    _ => None,
                })
                // A planned image the page model no longer has: the plan is not this document's.
                .ok_or(PdfError::new(ErrorCode::Internal))?;
            let m = Affine::new(model.geometry.user_to_display.0) * Affine::new(item.ctm.0);
            let rs = rects(pi, &page.regions);
            let clone = pi.needs_clone || pi.obj.is_none();
            if clone {
                if !pi.path.0.is_empty() {
                    return Err(refuse("redactionFormImageClone", page.page_index));
                }
                if plan
                    .stream_pages
                    .get(&pi.loc.stream)
                    .is_some_and(|p| p.len() > 1)
                {
                    return Err(refuse("redactionSharedContent", page.page_index));
                }
            }
            if let (true, Some(obj)) = (clone, pi.obj) {
                let span = pi.loc.start as usize..pi.loc.end as usize;
                cloned_names.push((page.page_index, obj, do_name(doc, pi.loc.stream, span)?));
            }
            let (enc, conv) = match pi.obj {
                Some(obj) => encode(
                    &stream_of(doc, obj)?,
                    item,
                    obj,
                    m,
                    &rs,
                    r.limits(),
                    &mut budget,
                )?,
                None => encode_inline(
                    doc,
                    pi.loc.stream,
                    pi.loc.start as usize..pi.loc.end as usize,
                    item,
                    m,
                    &rs,
                    page.page_index,
                )?,
            };
            report.images_erased += 1;
            report.color_converted += u32::from(conv);
            report.images_cloned += u32::from(pi.needs_clone);
            report.inline_converted += u32::from(pi.obj.is_none());
            let target = match (clone, pi.obj) {
                (false, Some(obj)) => Target::InPlace(obj),
                _ => Target::Clone {
                    page: page.page_index,
                    span: (pi.loc.stream, pi.loc.start as usize..pi.loc.end as usize),
                },
            };
            jobs.push(Job { target, enc });
        }
    }
    for (n, job) in jobs.into_iter().enumerate() {
        let (dict, data) = with_smask(w, job.enc);
        match job.target {
            Target::InPlace(obj) => {
                w.replace_stream(obj, NewStream { dict, data });
            }
            Target::Clone { page, span } => {
                let name = format!("FlkImg{n}");
                let new = w.add_object(dict_val(dict), Some(data));
                w.add_page_xobject(page as usize, &name, new)?;
                w.replace_span(span.0, span.1, format!("/{name} Do").as_bytes());
            }
        }
    }
    // An image every occurrence of which on the page was cloned must not stay reachable from
    // the page's resources (it holds the erased pixels).
    cloned_names.sort();
    cloned_names.dedup();
    for (page, obj, name) in cloned_names {
        if !survivors.contains(&(page, obj)) {
            w.drop_page_xobject(page as usize, &name)?;
        }
    }
    Ok(report)
}
