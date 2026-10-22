//! Native export (spec §4, §5, §8): one revision's working copy, its committed redactions and a
//! policy become held bytes plus an exact report. Pipeline order, each stage complete or a typed
//! refusal with no bytes:
//!
//! 1. raster mode leaves the native pipeline here: [`raster`] (eligibility `raster_plan`, pixels
//!    `raster_pixels`, file `raster_write`) depends on rendering only, never on the native
//!    stages below, and a native refusal never falls back to it;
//! 2. preflight ([`preflight`]): `FORM_APPEARANCE_MISSING` / `XFA_UNSUPPORTED`, unevaluable
//!    optional content, signed input policy, and redaction the plan cannot execute (pattern
//!    paint, soft-mask groups) refuse by name, with the redaction ids;
//! 3. visible annotations and all widgets drawn as Forms on their pages
//!    ([`crate::sanitize::interactive`]), hidden layers and off-page or clipped-out content
//!    removed ([`crate::sanitize::visibility`]);
//! 4. removal on one writer ([`crate::redaction::execute`]: Form clones, text, images with
//!    their masks, vectors), then the region fills painted last, outside every clip. Removal
//!    never depends on a fill; `fill: null` paints nothing;
//! 5. sanitization: the allowlist writer plus the counts of [`crate::sanitize::document`];
//! 6. write, then drop the resources no surviving content uses ([`crate::sanitize::prune`]);
//! 7. verify (§5.7): strict reparse, page count, no cluster meeting rule 1, every
//!    image under a region erased, no vector geometry under a region (our own fills aside).
//!
//! Every requested redaction is applied (listed in [`Prepared::applied`] with its effective
//! regions) or the whole export is refused: a stage error is attributed to the redactions it
//! can concern and no bytes leave. Report counts come from what the stages did.
use crate::Document;
use crate::document::{Binding, Committed, StoredRedaction};
use crate::redaction::plan::{RedactionPlan, bounds_of, glyph_boxes, text_coverage};
use crate::redaction::text::num;
use crate::redaction::{Executed, execute, image};
use crate::sanitize::document::scan;
use crate::sanitize::{interactive, prune, resources, text_semantics, visibility};
use crate::writer::Writer;
use foliark_model::{
    Capabilities, Capability, CapabilityReason, CapabilityStatus, EditsReport, ErrorCode,
    ExportMode, ExportPolicy, ExportReport, ExtractOptions, Flattened, InvisibleTextPolicy,
    KeptReport, Limits, Loss, LossCode, PdfError, Point, Rect, RedactionReport, SanitizedItem,
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
    /// Every committed redaction, applied: its page and the boxes it effectively took out (the
    /// requested rectangles grown to every whole cluster, image and vector removed for them).
    pub applied: Vec<AppliedRedaction>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppliedRedaction {
    pub id: String,
    pub page_index: u32,
    pub effective: Vec<Rect>,
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

/// An opaque rectangle per region with a fill, painted last on its page (spec §5.5). Redactions
/// with the same fill in a row share one `q` ... `Q`.
fn fills(doc: &Document, committed: &Committed) -> Result<BTreeMap<u32, String>, PdfError> {
    let mut out: BTreeMap<u32, (String, Option<[f64; 3]>)> = BTreeMap::new();
    let n = |v: f64| num(v, 4);
    for r in &committed.redactions {
        let geometry = doc.page_geometry(r.page_index as usize)?;
        let Some([cr, cg, cb]) = r.fill else {
            continue;
        };
        let (ops, open) = out.entry(r.page_index).or_default();
        if *open != r.fill {
            if open.is_some() {
                ops.push_str("Q\n");
            }
            let _ = writeln!(ops, "q {} {} {} rg", n(cr), n(cg), n(cb));
            *open = r.fill;
        }
        for q in &r.regions {
            // The display-to-user map only turns, flips and scales, so two opposite corners
            // give the rectangle.
            let user = |x, y| geometry.display_to_user.apply(Point { x, y });
            let (a, c) = (user(q.x, q.y), user(q.x + q.width, q.y + q.height));
            let _ = writeln!(
                ops,
                "{} {} {} {} re f",
                n(a.x.min(c.x)),
                n(a.y.min(c.y)),
                n((a.x - c.x).abs()),
                n((a.y - c.y).abs())
            );
        }
    }
    Ok(out
        .into_iter()
        .map(|(page, (mut ops, _))| {
            ops.push_str("Q\n");
            (page, ops)
        })
        .collect())
}

/// How far a region is shrunk for the output re-plan, so that the rectangles a cut leaves next
/// to it (rounded when written) do not count as geometry under it.
const SLACK: f64 = 0.01;

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
        let regions = page_regions(committed, p, 0.0);
        for c in out.extract_text(p, &opts)?.clusters.iter().flatten() {
            invisible += u32::from(c.flags & 1 != 0);
            if regions.iter().any(|r| text_coverage(&c.quad, *r).is_some()) {
                return Err(verify_failed());
            }
        }
    }
    if committed.redactions.is_empty() {
        return Ok(invisible);
    }
    // Images and vectors: plan the same regions (text ranges as plain regions) against the
    // output. What it still finds under a region must be erased or be one of our fills.
    let shrunk = Committed {
        edits: Vec::new(),
        redactions: committed
            .redactions
            .iter()
            .map(|r| StoredRedaction {
                regions: r.regions.iter().map(|g| shrink(g, SLACK)).collect(),
                must_remove: Vec::new(),
                ..r.clone()
            })
            .collect(),
    };
    let binding = Binding {
        doc_id: String::new(),
        generation: 0,
        revision: 0,
    };
    let after = RedactionPlan::build(&out, &shrunk, binding)?;
    if image::unerased(&out, &after)? > 0 {
        return Err(verify_failed());
    }
    for page in &after.pages {
        let ours = page_regions(committed, page.page_index as usize, 0.0);
        let is_fill = |b: &Rect| {
            let b = bounds_of(b);
            ours.iter()
                .any(|r| r.iter().zip(b).all(|(x, y)| (x - y).abs() < 5.0 * SLACK))
        };
        // A filled path or clip outline that merely overlaps a region by bounds is judged on its
        // geometry; anything else planned (a stroke, ink wholly inside) must not exist.
        let class = |v: &&crate::redaction::plan::PlannedVector| {
            v.class != crate::redaction::plan::VectorClass::Cut && !is_fill(&v.bounds)
        };
        if page.vectors.iter().any(|v| class(&v)) {
            return Err(verify_failed());
        }
        if crate::redaction::vector::page_leaks(&out, page, &is_fill)? {
            return Err(verify_failed());
        }
    }
    Ok(invisible)
}

fn page_regions(committed: &Committed, page: usize, grow: f64) -> Vec<[f64; 4]> {
    committed
        .redactions
        .iter()
        .filter(|r| r.page_index as usize == page)
        .flat_map(|r| r.regions.iter().map(|g| bounds_of(&shrink(g, -grow))))
        .collect()
}

fn shrink(r: &Rect, by: f64) -> Rect {
    if r.width <= 2.0 * by || r.height <= 2.0 * by {
        return *r;
    }
    Rect {
        x: r.x + by,
        y: r.y + by,
        width: r.width - 2.0 * by,
        height: r.height - 2.0 * by,
    }
}

/// A stage error names the redactions it can concern: the ones with planned work on the pages
/// the error names (all pages if it names none). An error that already names ids keeps them.
fn attribute(plan: &RedactionPlan, mut e: PdfError) -> PdfError {
    let d = e.details.get_or_insert_with(Default::default);
    if d.ids.is_none() {
        let mut ids: Vec<String> = plan
            .pages
            .iter()
            .filter(|p| d.pages.as_ref().is_none_or(|pg| pg.contains(&p.page_index)))
            .flat_map(|p| &p.regions)
            .filter(|r| r.clusters + r.images + r.vectors > 0)
            .map(|r| r.redaction.clone())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        d.ids = Some(ids);
    }
    e
}

/// The counts the stages produced must be the counts the plan disclosed: a stage that did less
/// than planned dropped work silently.
fn check_stages(plan: &RedactionPlan, x: &Executed) -> Result<(), PdfError> {
    let p = &plan.report;
    let ok = x.text.clusters_removed == p.clusters_removed
        && x.images.images_erased == p.images_erased
        && x.images.images_cloned == p.images_cloned
        && x.forms_cloned == p.forms_cloned
        // A planned cut may find its ink clear of the regions (`untouched`): every planned
        // entry must still be accounted for.
        && x.vectors.subpaths_removed + x.vectors.paths_cut + x.vectors.untouched
            == p.vector_subpaths_removed + p.vector_paths_cut
        && x.vectors.collateral.len() == p.collateral.len();
    if ok { Ok(()) } else { Err(verify_failed()) }
}

fn applied(plan: &RedactionPlan, committed: &Committed) -> Result<Vec<AppliedRedaction>, PdfError> {
    let mut out: Vec<AppliedRedaction> = Vec::new();
    for page in &plan.pages {
        for r in &page.regions {
            match out
                .iter_mut()
                .find(|a| a.id == r.redaction && a.page_index == page.page_index)
            {
                Some(a) => a.effective.push(r.effective),
                None => out.push(AppliedRedaction {
                    id: r.redaction.clone(),
                    page_index: page.page_index,
                    effective: vec![r.effective],
                }),
            }
        }
    }
    if out.len() != committed.redactions.len() {
        return Err(PdfError::new(ErrorCode::Internal));
    }
    Ok(out)
}

/// `committed` as pass 2 needs it on `staged`, the rewritten `doc`: a text redaction names the
/// glyphs it must remove by content-stream location, which pass 1 moved. Each is found again by
/// its position on the page; one pass 1 removed is gone and needs no entry.
fn remap(committed: &Committed, doc: &Document, staged: &Document) -> Result<Committed, PdfError> {
    let mut out = committed.clone();
    for page in committed
        .redactions
        .iter()
        .filter(|r| !r.must_remove.is_empty())
        .map(|r| r.page_index as usize)
        .collect::<std::collections::BTreeSet<_>>()
    {
        let before: std::collections::HashMap<_, _> = glyph_boxes(doc, page)?.into_iter().collect();
        let after = glyph_boxes(staged, page)?;
        for r in out
            .redactions
            .iter_mut()
            .filter(|r| r.page_index as usize == page)
        {
            let moved: Vec<_> = r
                .must_remove
                .iter()
                .filter_map(|id| before.get(id))
                .flat_map(|b| {
                    after
                        .iter()
                        .filter(move |(_, a)| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.05))
                        .map(|(id, _)| id.clone())
                })
                .collect();
            r.must_remove = moved;
        }
    }
    Ok(out)
}

/// Stage 6: the written file without the resource entries no surviving content uses (ADR 0007),
/// by a second write of the file read back. The count is the entries dropped.
fn drop_unused(bytes: Vec<u8>, limits: &Limits) -> Result<(Vec<u8>, u32), PdfError> {
    let out = Document::open_with_limits(bytes.clone(), None, false, limits.clone())?;
    let mut w = Writer::new(out.reader(), limits.clone());
    let n = prune::unused(out.reader(), &mut w)?;
    if n == 0 {
        return Ok((bytes, 0));
    }
    Ok((w.write()?, n))
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
    let inter = interactive::scan(reader, policy);
    let vis = visibility::scan_with(reader, policy.invisible_text == InvisibleTextPolicy::Remove)?;
    if let Some(b) = preflight::document_blockers(policy, &inter, &vis, removals.signatures).first()
    {
        return Err(b.error());
    }
    // Pass 1 (spec §5): flatten annotations and widgets, resolve layers and drop what lies off
    // the page, written out and reopened, so pass 2 plans and edits the content as it will be
    // output and never rewrites an operator span pass 1 already rewrote. Nothing to do: pass 2
    // runs on the input itself.
    let staged = if inter.is_empty() && vis.is_empty() {
        None
    } else {
        let mut w = Writer::new(reader, limits.clone());
        inter.apply(&mut w)?;
        vis.apply(&mut w);
        let bytes = w.write()?;
        Some(Document::open_with_limits(
            bytes,
            None,
            false,
            limits.clone(),
        )?)
    };
    let remapped;
    let (doc, committed) = match &staged {
        Some(s) => {
            remapped = remap(committed, doc, s)?;
            (s, &remapped)
        }
        None => (doc, committed),
    };
    let plan = RedactionPlan::build(doc, committed, binding)?;
    let mut blockers = preflight::plan_blockers(&plan);
    blockers.extend(text_semantics::mapping_blockers(doc, &plan)?);
    if let Some(b) = blockers.first() {
        return Err(b.error());
    }

    let mut w = Writer::new(doc.reader(), limits.clone());
    if staged.is_none() {
        inter.apply(&mut w)?;
        vis.apply(&mut w);
    }
    let done = execute(doc, &plan, &mut w).map_err(|e| attribute(&plan, e))?;
    check_stages(&plan, &done)?;
    let touched = plan.font_survivors.keys().copied().collect();
    let res = resources::rebuild(doc, &touched, &mut w)?;
    for (page, ops) in fills(doc, committed)? {
        w.append_page_content(page as usize, ops.as_bytes())?;
    }
    let (bytes, unused) = drop_unused(w.write()?, limits)?;
    let pages = doc.page_count();
    let invisible = verify(&bytes, pages, committed, limits)?;
    let applied = applied(&plan, committed)?;

    let mut removed = removals.removed;
    for (item, n) in [
        (SanitizedItem::Annotations, inter.annotations),
        (SanitizedItem::Widgets, inter.widgets),
        (SanitizedItem::OcgContent, vis.hidden_ops + inter.oc_hidden),
        (SanitizedItem::OffPage, vis.off_page_ops),
        (SanitizedItem::InvisibleText, vis.invisible_glyphs),
        (SanitizedItem::UnusedResources, unused),
    ] {
        if n > 0 {
            removed.insert(item, n);
        }
    }
    if vis.accessibility > 0 {
        *removed.entry(SanitizedItem::Structure).or_default() += vis.accessibility;
    }
    let (flat_annots, flat_widgets) = inter.flattened();
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
    add(LossCode::LinksRemoved, inter.links);
    add(LossCode::AnnotationsFlattened, flat_annots);
    add(LossCode::AnnotationsRemoved, inter.removed_visible);
    add(LossCode::FormsFlattened, flat_widgets);
    add(LossCode::LayersFlattened, vis.layers);
    add(LossCode::AccessibilityRemoved, vis.accessibility);
    add(LossCode::TextSearchLost, vis.invisible_glyphs);
    let mut inventory: Vec<String> = done
        .text
        .fonts_with_glyph_inventory
        .into_iter()
        .chain(res.fonts_with_glyph_inventory)
        .map(|(_, name)| name)
        .collect();
    inventory.sort_unstable();
    inventory.dedup();
    if unused > 0 {
        // Fonts dropped as unused are no longer in the file.
        let present = resources::font_names(&Document::open_with_limits(
            bytes.clone(),
            None,
            false,
            limits.clone(),
        )?);
        inventory.retain(|n| present.contains(n) || n.ends_with(" R"));
    }
    add(LossCode::FontGlyphInventoryRetained, inventory.len() as u32);
    let mut stage_loss = |code, count: u32, pages: Vec<u32>| {
        if count > 0 {
            merge(
                &mut losses,
                [Loss {
                    code,
                    count,
                    pages: Some(pages),
                }],
            );
        }
    };
    let on = |has: fn(&crate::redaction::plan::PagePlan) -> bool| -> Vec<u32> {
        plan.pages
            .iter()
            .filter(|p| has(p))
            .map(|p| p.page_index)
            .collect()
    };
    stage_loss(
        LossCode::ImageReencoded,
        done.images.images_erased,
        on(|p| !p.images.is_empty()),
    );
    let mut collateral_pages: Vec<u32> = done
        .vectors
        .collateral
        .iter()
        .map(|c| c.page_index)
        .collect();
    collateral_pages.dedup();
    stage_loss(
        LossCode::VectorCollateral,
        done.vectors.collateral.len() as u32,
        collateral_pages,
    );
    stage_loss(
        LossCode::VectorCutApproximated,
        done.vectors.approximated,
        on(|p| !p.vectors.is_empty()),
    );

    let redaction = RedactionReport {
        regions: plan.report.regions,
        clusters_removed: done.text.clusters_removed,
        images_erased: done.images.images_erased,
        images_cloned: done.images.images_cloned,
        forms_cloned: done.forms_cloned,
        vector_subpaths_removed: done.vectors.subpaths_removed,
        vector_paths_cut: done.vectors.paths_cut,
        collateral: done.vectors.collateral,
        shared_survivors: plan.report.shared_survivors.clone(),
    };
    let output_bytes = u32::try_from(bytes.len())
        .map_err(|_| PdfError::limit("maxOutputBytes", bytes.len() as u64, u64::from(u32::MAX)))?;
    let report = ExportReport {
        mode,
        pages: pages as u32,
        output_bytes,
        removed,
        flattened: Flattened {
            annotations: flat_annots,
            widgets: flat_widgets,
            ocg_layers: vis.layers,
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
    Ok(Prepared {
        bytes,
        report,
        applied,
    })
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
    let layers = if visibility::present(reader) {
        visibility::scan(reader)?
    } else {
        visibility::Scan::default()
    };
    let mut blockers =
        preflight::document_blockers(&policy, &interactive::scan(reader, &policy), &layers, 0);
    let redact = if committed.redactions.is_empty() {
        capability(Unknown, &[])
    } else {
        let plan = RedactionPlan::build(doc, committed, binding)?;
        let mut b = preflight::plan_blockers(&plan);
        b.extend(text_semantics::mapping_blockers(doc, &plan)?);
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
