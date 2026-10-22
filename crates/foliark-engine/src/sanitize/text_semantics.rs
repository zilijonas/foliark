//! Text semantics at export (spec §5 rules 1 and 6, §8.2; NEW-44).
//!
//! - Invisible OCR text (render mode 3 or 7) stays searchable under `invisibleText: keep` and
//!   goes under every region (rule 1: the redaction plan sees it like any glyph). `remove`
//!   drops all of it in the layer walk ([`super::visibility::scan_with`]).
//! - `ActualText`, `Alt` and `E` of marked-content property lists, inline or in `/Properties`,
//!   at any depth, are counted here and never copied (the writer's content re-emitter and its
//!   allowlist).
//! - `keepOcr` (ADR 0008): invisible text stays only where its glyph boxes lie over a drawn raster
//!   image ([`ImageCover`]), the shape of a scanner's OCR layer. Any other invisible show, and
//!   every one whose place cannot be established, goes like under `remove`.
//! - No false success: a glyph whose place on the page cannot be established (a singular CTM,
//!   font size 0) may be a hidden copy of a redacted word that the plan cannot see, so an export
//!   with regions on that page is refused.
use crate::Document;
use crate::export::preflight::Blocker;
use crate::interpret::{Item, ItemKind};
use crate::redaction::plan::RedactionPlan;
use foliark_model::{ErrorCode, Matrix, PdfError, Point, Quad};
use hayro::hayro_syntax::content::Instruction;
use hayro::hayro_syntax::object::{Dict, Object};
use hayro::hayro_syntax::page::Resources;

/// Marked-content property keys that carry document text.
pub const ACCESSIBILITY_KEYS: [&str; 3] = ["ActualText", "Alt", "E"];

/// Deeper than any real property list; past it nothing is counted (and the writer drops it).
const MAX_DEPTH: u8 = 32;

fn count(o: &Object<'_>, depth: u8) -> u32 {
    if depth > MAX_DEPTH {
        return 0;
    }
    match o {
        Object::Dict(d) => d
            .keys()
            .map(|k| {
                u32::from(ACCESSIBILITY_KEYS.contains(&k.as_str()))
                    + d.get::<Object<'_>>(k).map_or(0, |v| count(&v, depth + 1))
            })
            .sum(),
        Object::Array(a) => a.iter::<Object<'_>>().map(|v| count(&v, depth + 1)).sum(),
        _ => 0,
    }
}

/// Accessibility entries of one `BDC` instruction: its inline property dictionary, or the
/// `/Properties` entry it names.
pub fn accessibility_entries(ins: &Instruction<'_, '_>, res: &Resources<'_>) -> u32 {
    let mut ops = ins.operands();
    ops.next();
    match ops.next() {
        Some(Object::Name(n)) => res
            .properties
            .get::<Dict<'_>>(n)
            .map_or(0, |d| count(&Object::Dict(d), 0)),
        Some(o) => count(o, 0),
        None => 0,
    }
}

/// Whether a glyph quad covers a finite area (shoelace) a region can be tested against.
pub fn placed(q: &Quad) -> bool {
    let p = &q.0;
    let a = (0..4)
        .map(|i| p[i].x * p[(i + 1) % 4].y - p[(i + 1) % 4].x * p[i].y)
        .sum::<f64>()
        .abs()
        / 2.0;
    a.is_finite() && a > 1e-6
}

/// Pages with regions that draw a glyph without a place on the page: its quad has no area, so
/// no region can be said to cover it or not.
pub fn mapping_blockers(doc: &Document, plan: &RedactionPlan) -> Result<Vec<Blocker>, PdfError> {
    let mut b = Blocker {
        code: ErrorCode::InvalidArgument,
        feature: "textPositionUnknown",
        pages: Vec::new(),
        ids: Vec::new(),
        fields: Vec::new(),
    };
    for p in &plan.pages {
        let lost = doc
            .page_model(p.page_index as usize)?
            .items
            .iter()
            .any(|it| matches!(&it.kind, ItemKind::Glyph(g) if !placed(&g.quad_user)));
        if lost {
            b.pages.push(p.page_index);
            b.ids.extend(p.regions.iter().map(|r| r.redaction.clone()));
        }
    }
    b.ids.sort_unstable();
    b.ids.dedup();
    Ok(if b.pages.is_empty() { vec![] } else { vec![b] })
}

/// Share of a glyph box that must lie over drawn imagery for its invisible text to count as OCR.
const OCR_COVER: f64 = 0.8;
/// Sample grid per glyph box side (a union of images and clips is not a polygon to intersect).
const GRID: usize = 4;

/// The drawn raster images of one page: unit square inverse and visible bounds of each.
pub struct ImageCover {
    images: Vec<(Matrix, [f64; 4])>,
}

impl ImageCover {
    /// Images the page (and its Forms) draws, minus those hidden by optional content, clipped
    /// out or outside `crop` (`[x0, y0, x1, y1]`).
    pub fn of(items: &[Item], crop: [f64; 4]) -> Self {
        let images = items
            .iter()
            .filter(|it| !it.ocg_hidden)
            .filter_map(|it| {
                let ItemKind::Image(im) = &it.kind else {
                    return None;
                };
                let b = it.clip.map_or(crop, |c| {
                    [
                        c[0].max(crop[0]),
                        c[1].max(crop[1]),
                        c[2].min(crop[2]),
                        c[3].min(crop[3]),
                    ]
                });
                Some((im.ctm.inverse()?, b))
            })
            .collect();
        Self { images }
    }

    fn over(&self, p: Point) -> bool {
        self.images.iter().any(|(inv, b)| {
            let [a, bb, c, d, e, f] = inv.0;
            let (u, v) = (a * p.x + c * p.y + e, bb * p.x + d * p.y + f);
            (0.0..=1.0).contains(&u)
                && (0.0..=1.0).contains(&v)
                && (b[0]..=b[2]).contains(&p.x)
                && (b[1]..=b[3]).contains(&p.y)
        })
    }

    /// Whether at least [`OCR_COVER`] of the glyph box lies over an image. A box without area
    /// (singular CTM, size 0) has no place, so it is never covered.
    pub fn covers(&self, q: &Quad) -> bool {
        if !placed(q) {
            return false;
        }
        let [tl, tr, br, bl] = q.0;
        let lerp = |a: Point, b: Point, t: f64| Point {
            x: a.x + (b.x - a.x) * t,
            y: a.y + (b.y - a.y) * t,
        };
        let mut hit = 0;
        for i in 0..GRID {
            let (l, r) = (
                lerp(tl, bl, (i as f64 + 0.5) / GRID as f64),
                lerp(tr, br, (i as f64 + 0.5) / GRID as f64),
            );
            for j in 0..GRID {
                hit += usize::from(self.over(lerp(l, r, (j as f64 + 0.5) / GRID as f64)));
            }
        }
        hit as f64 >= OCR_COVER * (GRID * GRID) as f64
    }
}
