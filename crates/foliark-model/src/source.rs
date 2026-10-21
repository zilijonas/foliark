//! Where a piece of page content came from in the input file (spec §3 provenance, §5 redaction).
use serde::Serialize;
use ts_rs::TS;

/// A PDF indirect object id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, TS)]
#[ts(export, export_to = "../../../packages/sdk/src/generated/")]
pub struct ObjectId {
    pub num: u32,
    /// The generation number (`gen` is reserved in Rust 2024).
    #[serde(rename = "gen")]
    #[ts(rename = "gen")]
    pub generation: u16,
}
