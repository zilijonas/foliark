//! Native redaction (spec §5): `plan` decides what a committed redaction list takes out of the
//! working copy and discloses it before export; `text` removes the planned text clusters, `vector` the planned filled and stroked geometry.
pub mod plan;
pub mod text;
pub mod vector;
