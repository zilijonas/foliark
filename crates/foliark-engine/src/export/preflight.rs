//! Export preflight (spec §4, §5, §11): decide, before any byte is written, which pipeline
//! stages this build cannot run for a revision. A stage that is not built (optional-content flattening NEW-43, image erasure NEW-34/35, vector
//! subtraction NEW-36/37) is a typed refusal that names the feature and the pages, never a
//! silent drop. Annotation and widget flattening runs ([`crate::sanitize::interactive`]); its
//! refusals (`FORM_APPEARANCE_MISSING`, `XFA_UNSUPPORTED`) join the document blockers. Also the stable policy hash a plan is bound to.
use crate::reader::Document;
use crate::redaction::plan::{PagePlan, RedactionPlan};
use crate::sanitize::interactive::Scan;
use foliark_model::{
    ErrorCode, ErrorDetails, ExportMode, ExportPolicy, InvisibleTextPolicy, PdfError,
    SignedInputPolicy,
};
use hayro::hayro_syntax::object::Dict;

/// One reason a stage cannot run. `InvalidArgument` + `feature` is the engine's convention for
/// an op the spec defines but this build does not implement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    pub code: ErrorCode,
    pub feature: &'static str,
    pub pages: Vec<u32>,
    /// Field names (`FORM_APPEARANCE_MISSING`).
    pub fields: Vec<String>,
}

impl Blocker {
    fn not_built(feature: &'static str, pages: Vec<u32>) -> Self {
        Self {
            code: ErrorCode::InvalidArgument,
            feature,
            pages,
            fields: Vec::new(),
        }
    }

    pub fn error(&self) -> PdfError {
        PdfError::with(
            self.code,
            ErrorDetails {
                feature: Some(self.feature.into()),
                pages: (!self.pages.is_empty()).then(|| self.pages.clone()),
                fields: (!self.fields.is_empty()).then(|| self.fields.clone()),
                ..ErrorDetails::default()
            },
        )
    }
}

fn catalog<'a>(doc: &'a Document) -> Option<Dict<'a>> {
    let x = doc.pdf().xref();
    x.get::<Dict<'_>>(x.root_id())
}

/// Stages of the document that the working copy needs and this build cannot run, in pipeline
/// order (spec §5): signed input policy, form and annotation flattening, optional content.
pub fn document_blockers(
    doc: &Document,
    policy: &ExportPolicy,
    interactive: &Scan,
    signatures: u32,
) -> Vec<Blocker> {
    let mut out = Vec::new();
    if policy.signed_input == SignedInputPolicy::Refuse && signatures > 0 {
        out.push(Blocker {
            code: ErrorCode::SignedInput,
            feature: "signedInput",
            pages: Vec::new(),
            fields: Vec::new(),
        });
    }
    out.extend(interactive.blockers.iter().cloned());
    let cat = catalog(doc);
    if cat.is_some_and(|c| c.contains_key("OCProperties")) {
        out.push(Blocker {
            code: ErrorCode::OptionalContentUnsupported,
            feature: "flattenOptionalContent",
            pages: Vec::new(),
            fields: Vec::new(),
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
            fields: Vec::new(),
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
