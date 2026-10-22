//! Native export (spec §4, §5, §8): one revision's working copy, its committed redactions and a
//! policy become held bytes plus an exact report. Pipeline order, each stage complete or a typed
//! refusal with no bytes:
//!
//! 1. raster mode leaves the native pipeline here: [`raster`] (eligibility `raster_plan`, pixels
//!    `raster_pixels`, file `raster_write`) depends on rendering only, never on the native
//!    stages below, and a native refusal never falls back to it;
//! 2. preflight ([`preflight`]): widget / annotation flattening (NEW-42), optional-content
//!    flattening (NEW-43), signed input policy, image erasure (NEW-34/35) and vector cutting
//!    (NEW-36/37) refuse by name. `annotations: remove` is built (the allowlist writer drops
//!    every annotation), so it needs no flattening stage;
//! 3. text redaction (`redact_text`) and region fills, through the writer;
//! 4. sanitization: the allowlist writer plus the counts of [`crate::sanitize::document`];
//! 5. write; 6. verify (§5.7): strict reparse, page count, no cluster meeting rule 1.
use crate::Document;
use crate::document::{Binding, Committed};
use crate::redaction::plan::{RedactionPlan, bounds_of, text_coverage};
use crate::redaction::text::redact_text;
use crate::sanitize::document::scan;
use crate::writer::Writer;
use foliark_model::{
    Capabilities, Capability, CapabilityReason, CapabilityStatus, EditsReport, ErrorCode,
    ExportMode, ExportPolicy, ExportReport, ExtractOptions, Flattened, KeptReport, Limits, Loss,
    LossCode, PdfError, Point, SanitizedItem,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;

pub mod preflight;
pub mod raster;
pub mod raster_pixels;
pub mod raster_plan;
pub mod raster_write;
use preflight::Blocker;

/// The plan the engine holds: what `export` checks and hands over. `bytes` is `None` once taken.
pub(crate) struct Held {
    pub id: String,
    pub binding: Binding,
    pub mode: ExportMode,
    pub fonts: u32,
    pub bytes: Option<Vec<u8>>,
}

pub struct Prepared {
    pub bytes: Vec<u8>,
    pub report: ExportReport,
}

pub(crate) fn verify_failed() -> PdfError {
    PdfError::new(ErrorCode::RedactVerifyFailed)
}

fn merge(losses: &mut Vec<Loss>, add: impl IntoIterator<Item = Loss>) {
    for l in add {
        match losses.iter_mut().find(|m| m.code == l.code) {
            Some(m) => {
                m.count += l.count;
                if let Some(p) = l.pages {
                    let pages = m.pages.get_or_insert_with(Vec::new);
                    pages.extend(p);
                    pages.sort_unstable();
                    pages.dedup();
                }
            }
            None => losses.push(l),
        }
    }
}

/// An opaque rectangle per region, painted last on its page (spec §5.5). `fill: None` is black.
fn fills(doc: &Document, committed: &Committed) -> Result<BTreeMap<u32, String>, PdfError> {
    let mut out: BTreeMap<u32, String> = BTreeMap::new();
    for r in &committed.redactions {
        let geometry = doc.page_geometry(r.page_index as usize)?;
        let [cr, cg, cb] = r.fill.unwrap_or([0.0; 3]);
        let ops = out.entry(r.page_index).or_default();
        for q in &r.regions {
            let corners = [
                (q.x, q.y),
                (q.x + q.width, q.y),
                (q.x + q.width, q.y + q.height),
                (q.x, q.y + q.height),
            ];
            let _ = write!(ops, "q {cr:.4} {cg:.4} {cb:.4} rg");
            for (i, (x, y)) in corners.into_iter().enumerate() {
                let p = geometry.display_to_user.apply(Point { x, y });
                let _ = write!(
                    ops,
                    " {:.4} {:.4} {}",
                    p.x,
                    p.y,
                    if i == 0 { "m" } else { "l" }
                );
            }
            ops.push_str(" h f Q\n");
        }
    }
    Ok(out)
}

/// Spec §5.7 on the output, plus the invisible-cluster count of the report. `Err` = no output.
fn verify(
    bytes: &[u8],
    pages: usize,
    committed: &Committed,
    limits: &Limits,
) -> Result<u32, PdfError> {
    let out = Document::open_with_limits(bytes.to_vec(), None, false, limits.clone())
        .map_err(|_| verify_failed())?;
    if out.page_count() != pages {
        return Err(verify_failed());
    }
    let opts = ExtractOptions {
        chars: true,
        include_ocg_hidden: true,
    };
    let mut invisible = 0u32;
    for p in 0..pages {
        let regions: Vec<[f64; 4]> = committed
            .redactions
            .iter()
            .filter(|r| r.page_index as usize == p)
            .flat_map(|r| r.regions.iter().map(bounds_of))
            .collect();
        for c in out.extract_text(p, &opts)?.clusters.iter().flatten() {
            invisible += u32::from(c.flags & 1 != 0);
            if regions.iter().any(|r| text_coverage(&c.quad, *r).is_some()) {
                return Err(verify_failed());
            }
        }
    }
    Ok(invisible)
}

/// Run the native pipeline for `doc`, the working copy at `binding`.
pub fn prepare(
    doc: &Document,
    committed: &Committed,
    binding: Binding,
    mode: ExportMode,
    policy: &ExportPolicy,
    limits: &Limits,
) -> Result<Prepared, PdfError> {
    if mode == ExportMode::Raster {
        return raster::run(doc, committed, binding, policy, limits);
    }
    let reader = doc.reader();
    let removals = scan(reader)?;
    let annots = preflight::annotations(reader);
    if let Some(b) =
        preflight::document_blockers(reader, policy, &annots, removals.signatures).first()
    {
        return Err(b.error());
    }
    let plan = RedactionPlan::build(doc, committed, binding)?;
    if let Some(b) = preflight::plan_blockers(&plan).first() {
        return Err(b.error());
    }

    let mut w = Writer::new(reader, limits.clone());
    let text = redact_text(doc, &plan, &mut w)?;
    for (page, ops) in fills(doc, committed)? {
        w.append_page_content(page as usize, ops.as_bytes())?;
    }
    let bytes = w.write()?;
    let pages = doc.page_count();
    let invisible = verify(&bytes, pages, committed, limits)?;

    let mut removed = removals.removed;
    if annots.total > 0 {
        removed.insert(SanitizedItem::Annotations, annots.total);
    }
    let mut losses = removals.losses;
    let mut add = |code, count| {
        if count > 0 {
            merge(
                &mut losses,
                [Loss {
                    code,
                    count,
                    pages: None,
                }],
            );
        }
    };
    add(LossCode::LinksRemoved, annots.links);
    if policy.annotations == foliark_model::AnnotationPolicy::Remove {
        add(LossCode::AnnotationsRemoved, annots.visible);
    }
    let mut inventory: Vec<String> = text
        .fonts_with_glyph_inventory
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    inventory.sort_unstable();
    inventory.dedup();
    add(LossCode::FontGlyphInventoryRetained, inventory.len() as u32);
    merge(&mut losses, plan.losses.clone());

    let mut redaction = plan.report.clone();
    redaction.clusters_removed = text.clusters_removed;
    let output_bytes = u32::try_from(bytes.len())
        .map_err(|_| PdfError::limit("maxOutputBytes", bytes.len() as u64, u64::from(u32::MAX)))?;
    let report = ExportReport {
        mode,
        pages: pages as u32,
        output_bytes,
        removed,
        flattened: Flattened {
            annotations: 0,
            widgets: 0,
            ocg_layers: 0,
        },
        redaction,
        edits: EditsReport {
            applied: committed.edits.len() as u32,
            approximate: 0,
        },
        kept: KeptReport {
            invisible_text_clusters: invisible,
            fonts_with_glyph_inventory: inventory,
        },
        losses,
    };
    Ok(Prepared { bytes, report })
}

fn capability(status: CapabilityStatus, blockers: &[Blocker]) -> Capability {
    Capability {
        status,
        reasons: blockers
            .iter()
            .map(|b| CapabilityReason {
                code: b.code,
                pages: (!b.pages.is_empty()).then(|| b.pages.clone()),
            })
            .collect(),
    }
}

/// Spec §11. `native_ready` / `raster_ready`: a plan of that mode is held for this revision (the
/// only proof of support). Otherwise export is `unsupported` only when preflight already knows a
/// stage refuses (default policy), else `unknown`; raster never depends on native blockers.
pub(crate) fn capabilities(
    doc: &Document,
    committed: &Committed,
    binding: Binding,
    (native_ready, raster_ready): (bool, bool),
) -> Result<Capabilities, PdfError> {
    use CapabilityStatus::{Supported, Unknown, Unsupported};
    let reader = doc.reader();
    let policy = ExportPolicy::default();
    let mut blockers =
        preflight::document_blockers(reader, &policy, &preflight::annotations(reader), 0);
    let redact = if committed.redactions.is_empty() {
        capability(Unknown, &[])
    } else {
        let b = preflight::plan_blockers(&RedactionPlan::build(doc, committed, binding)?);
        let c = capability(if b.is_empty() { Supported } else { Unsupported }, &b);
        blockers.extend(b);
        c
    };
    finish(redact, &blockers, native_ready, raster_ready)
}

fn finish(
    redact: Capability,
    blockers: &[Blocker],
    native_ready: bool,
    raster_ready: bool,
) -> Result<Capabilities, PdfError> {
    use CapabilityStatus::{Supported, Unknown, Unsupported};
    let native = if native_ready {
        capability(Supported, &[])
    } else if blockers.is_empty() {
        capability(Unknown, &[])
    } else {
        capability(Unsupported, blockers)
    };
    Ok(Capabilities {
        extract: capability(Supported, &[]),
        render: capability(Supported, &[]),
        edit: capability(Unknown, &[]),
        redact,
        export_native: native,
        export_raster: capability(if raster_ready { Supported } else { Unknown }, &[]),
    })
}
