//! Export preflight (spec §4, §5, §11): decide, before any byte is written, which pipeline
//! stages this build cannot run for a revision. A stage that is not built (annotation and widget
//! flattening NEW-42, optional-content flattening NEW-43, image erasure NEW-34/35, vector
//! subtraction NEW-36/37) is a typed refusal that names the feature and the pages, never a
//! silent drop. Also the stable policy hash a plan is bound to.
use crate::reader::Document;
use crate::redaction::plan::{PagePlan, RedactionPlan};
use foliark_model::{
    AnnotationPolicy, ErrorCode, ErrorDetails, ExportMode, ExportPolicy, InvisibleTextPolicy,
    PdfError, SignedInputPolicy,
};
use hayro::hayro_syntax::object::{Array, Dict, Name};

/// One reason a stage cannot run. `InvalidArgument` + `feature` is the engine's convention for
/// an op the spec defines but this build does not implement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    pub code: ErrorCode,
    pub feature: &'static str,
    pub pages: Vec<u32>,
}

impl Blocker {
    fn not_built(feature: &'static str, pages: Vec<u32>) -> Self {
        Self {
            code: ErrorCode::InvalidArgument,
            feature,
            pages,
        }
    }

    pub fn error(&self) -> PdfError {
        PdfError::with(
            self.code,
            ErrorDetails {
                feature: Some(self.feature.into()),
                pages: (!self.pages.is_empty()).then(|| self.pages.clone()),
                ..ErrorDetails::default()
            },
        )
    }
}

/// What the pages' annotation arrays hold, by the action spec §8.2 takes on each.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Annots {
    /// Non-widget annotations (all are removed from a native export).
    pub total: u32,
    pub links: u32,
    /// Visible with a normal appearance: `flatten` must draw them, `remove` drops them.
    pub visible: u32,
    pub visible_pages: Vec<u32>,
    pub widget_pages: Vec<u32>,
}

const HIDDEN: u32 = 2;
const NO_VIEW: u32 = 32;

pub fn annotations(doc: &Document) -> Annots {
    let mut out = Annots::default();
    for (i, page) in doc.pdf().pages().iter().enumerate() {
        let Some(list) = page.raw().get::<Array<'_>>("Annots") else {
            continue;
        };
        let raw = list.raw_iter().count() as u32;
        let (mut widgets, mut visible) = (0u32, 0u32);
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
                    visible += u32::from(shown && drawn);
                }
            }
        }
        out.total += raw.saturating_sub(widgets);
        out.visible += visible;
        if visible > 0 {
            out.visible_pages.push(i as u32);
        }
        if widgets > 0 {
            out.widget_pages.push(i as u32);
        }
    }
    out
}

fn catalog<'a>(doc: &'a Document) -> Option<Dict<'a>> {
    let x = doc.pdf().xref();
    x.get::<Dict<'_>>(x.root_id())
}

/// Stages of the document that the working copy needs and this build cannot run, in pipeline
/// order (spec §5): signed input policy, widget and annotation flattening, optional content.
pub fn document_blockers(
    doc: &Document,
    policy: &ExportPolicy,
    annots: &Annots,
    signatures: u32,
) -> Vec<Blocker> {
    let mut out = Vec::new();
    if policy.signed_input == SignedInputPolicy::Refuse && signatures > 0 {
        out.push(Blocker {
            code: ErrorCode::SignedInput,
            feature: "signedInput",
            pages: Vec::new(),
        });
    }
    let cat = catalog(doc);
    let fields = cat
        .as_ref()
        .and_then(|c| c.get::<Dict<'_>>("AcroForm"))
        .and_then(|f| f.get::<Array<'_>>("Fields"))
        .is_some_and(|f| f.raw_iter().next().is_some());
    if fields || !annots.widget_pages.is_empty() {
        out.push(Blocker::not_built(
            "flattenWidgets",
            annots.widget_pages.clone(),
        ));
    }
    if policy.annotations == AnnotationPolicy::Flatten && annots.visible > 0 {
        out.push(Blocker::not_built(
            "flattenAnnotations",
            annots.visible_pages.clone(),
        ));
    }
    if cat.is_some_and(|c| c.contains_key("OCProperties")) {
        out.push(Blocker {
            code: ErrorCode::OptionalContentUnsupported,
            feature: "flattenOptionalContent",
            pages: Vec::new(),
        });
    }
    if policy.invisible_text == InvisibleTextPolicy::Remove {
        out.push(Blocker::not_built("invisibleTextRemove", Vec::new()));
    }
    out
}

/// Redaction work the plan found that no executor exists for yet: erasing image pixels and
/// cutting vectors. Text and region fills run now.
pub fn plan_blockers(plan: &RedactionPlan) -> Vec<Blocker> {
    let pages = |has: fn(&PagePlan) -> bool| {
        plan.pages
            .iter()
            .filter(|p| has(p))
            .map(|p| p.page_index)
            .collect::<Vec<_>>()
    };
    let mut out = Vec::new();
    let images = pages(|p| !p.images.is_empty());
    if !images.is_empty() {
        out.push(Blocker::not_built("redactionImages", images));
    }
    let vectors = pages(|p| !p.vectors.is_empty());
    if !vectors.is_empty() {
        out.push(Blocker::not_built("redactionVectors", vectors));
    }
    let patterns = pages(|p| !p.patterns.is_empty());
    if !patterns.is_empty() {
        out.push(Blocker {
            code: ErrorCode::RedactPatternUnsupported,
            feature: "redactionPatterns",
            pages: patterns,
        });
    }
    out
}

/// Stable hash (FNV-1a 64, 16 hex digits) of the normalized policy: the same effective policy
/// hashes the same whatever fields the caller spelled out. Raster settings only count in
/// raster mode, where they change the bytes.
pub fn policy_hash(mode: ExportMode, p: &ExportPolicy) -> String {
    let mut s = format!(
        "{mode:?}|{:?}|{:?}|{:?}",
        p.annotations, p.signed_input, p.invisible_text
    );
    if mode == ExportMode::Raster {
        s += &format!("|{}|{:?}", p.raster.acknowledge_losses, p.raster.scale);
    }
    let h = s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{h:016x}")
}
