//! Export preflight (spec §4, §5, §11): decide, before any byte is written, which pipeline
//! stages this build cannot run for a revision. A redaction the plan cannot execute (pattern
//! paint, soft-mask groups) is a typed refusal that names the feature, the pages and the
//! redaction ids, never a silent drop. Annotation and widget flattening
//! ([`crate::sanitize::interactive`]) and optional-content flattening
//! ([`crate::sanitize::visibility`]) run; their refusals (`FORM_APPEARANCE_MISSING`,
//! `XFA_UNSUPPORTED`, `OPTIONAL_CONTENT_UNSUPPORTED`) join the document blockers. Also the
//! stable policy hash a plan is bound to.
use crate::redaction::plan::{PagePlan, RedactionPlan};
use crate::sanitize::{interactive, visibility};
use foliark_model::{
    ErrorCode, ErrorDetails, ExportMode, ExportPolicy, InvisibleTextPolicy, PdfError,
    SignedInputPolicy,
};

/// One reason a stage cannot run. `InvalidArgument` + `feature` is the engine's convention for
/// an op the spec defines but this build does not implement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    pub code: ErrorCode,
    pub feature: &'static str,
    pub pages: Vec<u32>,
    /// The redactions the stage cannot execute (empty for document-level blockers).
    pub ids: Vec<String>,
    /// Field names (`FORM_APPEARANCE_MISSING`).
    pub fields: Vec<String>,
}

impl Blocker {
    fn not_built(feature: &'static str, pages: Vec<u32>) -> Self {
        Self {
            code: ErrorCode::InvalidArgument,
            feature,
            pages,
            ids: Vec::new(),
            fields: Vec::new(),
        }
    }

    pub fn error(&self) -> PdfError {
        PdfError::with(
            self.code,
            ErrorDetails {
                feature: Some(self.feature.into()),
                pages: (!self.pages.is_empty()).then(|| self.pages.clone()),
                ids: (!self.ids.is_empty()).then(|| self.ids.clone()),
                fields: (!self.fields.is_empty()).then(|| self.fields.clone()),
                ..ErrorDetails::default()
            },
        )
    }
}

/// Stages of the document that the working copy needs and this build cannot run, in pipeline
/// order (spec §5): signed input policy, form and annotation flattening, optional content.
pub fn document_blockers(
    policy: &ExportPolicy,
    interactive: &interactive::Scan,
    layers: &visibility::Scan,
    signatures: u32,
) -> Vec<Blocker> {
    let mut out = Vec::new();
    if policy.signed_input == SignedInputPolicy::Refuse && signatures > 0 {
        out.push(Blocker {
            code: ErrorCode::SignedInput,
            feature: "signedInput",
            pages: Vec::new(),
            ids: Vec::new(),
            fields: Vec::new(),
        });
    }
    out.extend(interactive.blockers.iter().cloned());
    out.extend(layers.blockers.iter().cloned());
    if policy.invisible_text == InvisibleTextPolicy::Remove {
        out.push(Blocker::not_built("invisibleTextRemove", Vec::new()));
    }
    out
}

/// Redaction work the plan found that no executor can carry out: paint of a tiling pattern
/// over a region (its cell is not interpreted) and content under a soft mask (the mask group is
/// not in the model). Each names the redactions whose regions reach it: refusing is atomic and
/// attributable, nothing is skipped.
pub fn plan_blockers(plan: &RedactionPlan) -> Vec<Blocker> {
    type Pick = fn(&PagePlan) -> &[u32];
    let stages: [(ErrorCode, &'static str, Pick); 2] = [
        (
            ErrorCode::RedactPatternUnsupported,
            "redactionPatterns",
            |p| &p.pattern_regions,
        ),
        (ErrorCode::InvalidArgument, "redactionSoftMaskGroup", |p| {
            &p.soft_masks
        }),
    ];
    let mut out = Vec::new();
    for (code, feature, pick) in stages {
        let mut b = Blocker {
            code,
            feature,
            pages: Vec::new(),
            ids: Vec::new(),
            fields: Vec::new(),
        };
        for p in &plan.pages {
            let hit = pick(p);
            if !hit.is_empty() {
                b.pages.push(p.page_index);
                b.ids
                    .extend(hit.iter().map(|&i| p.regions[i as usize].redaction.clone()));
            }
        }
        b.ids.sort_unstable();
        b.ids.dedup();
        if !b.pages.is_empty() {
            out.push(b);
        }
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
