//! Native redaction (spec §5): `plan` decides what a committed redaction list takes out of the
//! working copy and discloses it before export; `text` removes the planned text clusters, `image` the
//! planned image samples, `vector` the planned filled and stroked geometry (`execute` runs them in order); `forms` clones the Forms and content streams that
//! several occurrences share, so those three only ever write into the occurrence they act on.
use crate::Document;
use crate::writer::Writer;
use foliark_model::PdfError;
use image::ImageReport;
use text::TextReport;
use vector::VectorReport;

mod forms;
mod geometry;
pub mod image;
pub mod masks;
pub mod plan;
pub mod text;
pub mod vector;

/// What the stages of [`execute`] actually did, to be reported instead of the planned counts.
#[derive(Debug, Clone, Default)]
pub struct Executed {
    pub text: TextReport,
    pub images: ImageReport,
    pub vectors: VectorReport,
    /// Form clones created (one per cloned occurrence).
    pub forms_cloned: u32,
}

/// Spec §5 removal on one writer, in order: Form clones, text, images (with their masks), vectors.
/// Fills are not part of it: they go on afterwards and removal never depends on them. The stages
/// write disjoint operators of each stream, so their order is not observable in the output. A
/// stage `Err` leaves the writer partly written: drop it.
pub fn execute(
    doc: &Document,
    plan: &plan::RedactionPlan,
    w: &mut Writer<'_>,
) -> Result<Executed, PdfError> {
    let targets = forms::Targets::new(doc, plan)?;
    targets.apply(doc, plan, w)?;
    Ok(Executed {
        text: text::redact_text(doc, plan, w)?,
        images: image::redact_images(doc, plan, w)?,
        vectors: vector::redact_vectors(doc, plan, w)?,
        forms_cloned: targets.forms_cloned(),
    })
}
