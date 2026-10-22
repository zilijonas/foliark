//! Native redaction (spec §5): `plan` decides what a committed redaction list takes out of the
//! working copy and discloses it before export; `text` removes the planned text clusters, `image` the
//! planned image samples, `vector` the planned filled and stroked geometry; `forms` clones the Forms and content streams that
//! several occurrences share, so those three only ever write into the occurrence they act on.
mod forms;
pub mod image;
pub mod masks;
pub mod plan;
pub mod text;
pub mod vector;
