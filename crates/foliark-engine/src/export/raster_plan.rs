//! Raster eligibility (spec §7; NEW-54): what a raster export of one revision needs and whether
//! this build can do it, decided before any page is drawn. It depends on rendering, not on the
//! native pipeline: optional content, vector art, patterns and widgets are drawn, so none of the
//! native blockers of [`super::preflight`] applies, and a native refusal never turns into a
//! raster export by itself (the caller asks for `mode: raster` and acknowledges the losses).
//!
//! Refusals here, in order: `CONSENT_REQUIRED`, `INVALID_ARGUMENT` (scale), `SIGNED_INPUT`
//! (policy `refuse`), `LIMIT_EXCEEDED` (a page at `scale` over the render side or pixel limit).
//! `RENDER_INCOMPLETE {pages}` is the fifth: completeness is a property of the drawing, so the
//! pixel job reports it page by page ([`incomplete`]).
use crate::Document;
use crate::document::{Binding, Committed};
use crate::limits;
use crate::redaction::plan::RedactionPlan;
use crate::sanitize::document::{DocumentRemovals, scan};
use foliark_model::{
    AnnotationPolicy, ErrorCode, ErrorDetails, ExportPolicy, Limits, Loss, LossCode, PdfError,
    Rect, SanitizedItem, SignedInputPolicy,
};
use hayro::hayro_syntax::object::{Array, Dict, Name};

/// Spec §1.3: each rendered side is at most 16384 px.
const MAX_SIDE: u64 = 16384;

/// One page of the output: its display size in points and the pixel grid at the policy scale.
#[derive(Debug, Clone, PartialEq)]
pub struct PageSpec {
    pub width_pt: f64,
    pub height_pt: f64,
    pub width_px: u32,
    pub height_px: u32,
}

/// A display-space rectangle whose touched pixels become `fill` (sRGB bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Burn {
    pub rect: Rect,
    pub fill: [u8; 3],
}

/// What the raster export draws and what it drops, as counted on the working copy.
pub struct RasterPlan {
    pub scale: f64,
    /// Draw annotation appearances (policy `flatten`).
    pub annotations: bool,
    pub pages: Vec<PageSpec>,
    /// Per page, in painting order: requested regions, then the whole clusters they cut.
    pub burns: Vec<Vec<Burn>>,
    pub regions: u32,
    pub removals: DocumentRemovals,
    pub annots: Annots,
    pub widgets: u32,
    pub ocg_layers: u32,
}

/// What the pages' annotation arrays hold, by the action spec §8.2 takes on each.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Annots {
    /// Non-widget annotations (the fresh file carries none).
    pub total: u32,
    pub links: u32,
    /// Visible with a normal appearance: `flatten` draws them, `remove` drops them.
    pub visible: u32,
}

const HIDDEN: u32 = 2;
const NO_VIEW: u32 = 32;

fn annotations(doc: &crate::reader::Document) -> Annots {
    let mut out = Annots::default();
    for page in doc.pdf().pages().iter() {
        let Some(list) = page.raw().get::<Array<'_>>("Annots") else {
            continue;
        };
        let raw = list.raw_iter().count() as u32;
        let mut widgets = 0u32;
        for a in list.iter::<Dict<'_>>() {
            let kind = a.get::<Name<'_>>("Subtype");
            match kind.as_ref().map(|n| n.as_str()) {
                Some("Widget") => widgets += 1,
                Some("Link") => out.links += 1,
                Some("Popup") => {}
                _ => {
                    let shown = a.get::<u32>("F").unwrap_or(0) & (HIDDEN | NO_VIEW) == 0;
                    let drawn = a
                        .get::<Dict<'_>>("AP")
                        .is_some_and(|ap| ap.contains_key("N"));
                    out.visible += u32::from(shown && drawn);
                }
            }
        }
        out.total += raw.saturating_sub(widgets);
    }
    out
}

fn byte(c: f64) -> u8 {
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Spec §7: a `null` fill erases to WHITE in raster mode (native paints black); any other fill
/// is its colour. The region is always erased.
fn fill(f: Option<[f64; 3]>) -> [u8; 3] {
    f.unwrap_or([1.0; 3]).map(byte)
}

fn widgets(doc: &crate::reader::Document) -> u32 {
    let is_widget = |a: &Dict<'_>| {
        a.get::<Name<'_>>("Subtype")
            .is_some_and(|n| n.as_str() == "Widget")
    };
    doc.pdf()
        .pages()
        .iter()
        .filter_map(|p| p.raw().get::<Array<'_>>("Annots"))
        .flat_map(|a| a.iter::<Dict<'_>>())
        .filter(is_widget)
        .count() as u32
}

fn ocg_layers(doc: &crate::reader::Document) -> u32 {
    let x = doc.pdf().xref();
    x.get::<Dict<'_>>(x.root_id())
        .and_then(|c| c.get::<Dict<'_>>("OCProperties"))
        .and_then(|p| p.get::<Array<'_>>("OCGs"))
        .map_or(0, |a| a.raw_iter().count() as u32)
}

impl RasterPlan {
    /// `doc` is the working copy at `binding` (edits applied). Nothing is drawn.
    pub fn build(
        doc: &Document,
        committed: &Committed,
        binding: Binding,
        policy: &ExportPolicy,
        limits: &Limits,
    ) -> Result<Self, PdfError> {
        if !policy.raster.acknowledge_losses {
            return Err(PdfError::new(ErrorCode::ConsentRequired));
        }
        let scale = policy.raster.scale;
        if !(scale.is_finite() && scale > 0.0) {
            return Err(PdfError::new(ErrorCode::InvalidArgument));
        }
        let removals = scan(doc.reader())?;
        if policy.signed_input == SignedInputPolicy::Refuse && removals.signatures > 0 {
            return Err(PdfError::with(
                ErrorCode::SignedInput,
                ErrorDetails {
                    feature: Some("signedInput".into()),
                    ..ErrorDetails::default()
                },
            ));
        }
        let mut pages = Vec::new();
        for i in 0..doc.page_count() {
            let g = doc.page_geometry(i)?;
            let side = |len: f64| (len * scale).round().max(1.0);
            let (w, h) = (side(g.width), side(g.height));
            if w > MAX_SIDE as f64 || h > MAX_SIDE as f64 {
                return Err(PdfError::limit("maxRenderSide", w.max(h) as u64, MAX_SIDE));
            }
            limits::check(
                "maxRenderPixels",
                (w * h) as u64,
                limits.max_render_pixels.into(),
            )?;
            pages.push(PageSpec {
                width_pt: g.width,
                height_pt: g.height,
                width_px: w as u32,
                height_px: h as u32,
            });
        }

        let mut burns = vec![Vec::new(); pages.len()];
        let redaction = RedactionPlan::build(doc, committed, binding)?;
        for r in &committed.redactions {
            burns[r.page_index as usize].extend(r.regions.iter().map(|&rect| Burn {
                rect,
                fill: fill(r.fill),
            }));
        }
        // Spec §5.1 removes whole clusters; a glyph the region only grazes must not survive as a
        // sliver of ink, so the cluster's box is burned with its region's fill.
        for page in &redaction.pages {
            for c in &page.clusters {
                let fill = c
                    .regions
                    .first()
                    .and_then(|&i| page.regions.get(i as usize))
                    .map(|r| r.fill);
                burns[page.page_index as usize].push(Burn {
                    rect: c.quad.bounds(),
                    fill: self::fill(fill.flatten()),
                });
            }
        }
        Ok(Self {
            scale,
            annotations: policy.annotations == AnnotationPolicy::Flatten,
            pages,
            regions: committed
                .redactions
                .iter()
                .map(|r| r.regions.len() as u32)
                .sum(),
            burns,
            annots: annotations(doc.reader()),
            widgets: widgets(doc.reader()),
            ocg_layers: ocg_layers(doc.reader()),
            removals,
        })
    }

    /// Spec §7 losses: the two every raster export has, plus each §8 item present.
    pub fn losses(&self) -> Vec<Loss> {
        let n = self.pages.len() as u32;
        let mut out = vec![
            Loss {
                code: LossCode::TextSearchLost,
                count: n,
                pages: None,
            },
            Loss {
                code: LossCode::VectorRasterized,
                count: n,
                pages: None,
            },
        ];
        out.extend(self.removals.losses.iter().cloned());
        let a = &self.annots;
        let mut add = |code, count| {
            if count > 0 {
                out.push(Loss {
                    code,
                    count,
                    pages: None,
                });
            }
        };
        add(LossCode::LinksRemoved, a.links);
        if self.annotations {
            add(LossCode::AnnotationsFlattened, a.visible);
            add(LossCode::FormsFlattened, self.widgets);
        } else {
            add(LossCode::AnnotationsRemoved, a.visible + self.widgets);
        }
        add(LossCode::LayersFlattened, self.ocg_layers);
        out
    }

    /// `ExportReport.removed`: everything the fresh file does not carry.
    pub fn removed(&self) -> std::collections::BTreeMap<SanitizedItem, u32> {
        let mut out = self.removals.removed.clone();
        if self.annots.total > 0 {
            out.insert(SanitizedItem::Annotations, self.annots.total);
        }
        if self.widgets > 0 {
            out.insert(SanitizedItem::Widgets, self.widgets);
        }
        out
    }
}

/// `RENDER_INCOMPLETE` naming every page that drew with a warning (spec §7).
pub fn incomplete(pages: &[u32]) -> PdfError {
    PdfError::with(
        ErrorCode::RenderIncomplete,
        ErrorDetails {
            pages: Some(pages.to_vec()),
            ..ErrorDetails::default()
        },
    )
}
