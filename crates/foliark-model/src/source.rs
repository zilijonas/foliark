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

/// A byte range of one decoded content stream: the operator and its operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, TS)]
#[ts(export, export_to = "../../../packages/sdk/src/generated/")]
pub struct SourceLoc {
    pub stream: ObjectId,
    pub start: u32,
    pub end: u32,
}

/// One Form XObject (or annotation appearance, pattern, Type3 procedure) on the way down to
/// an item. `occurrence` counts how often `form` was drawn before inside the same parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, TS)]
#[ts(export, export_to = "../../../packages/sdk/src/generated/")]
pub struct InstanceStep {
    pub form: ObjectId,
    pub occurrence: u32,
}

/// The Form instances an item is drawn through, outermost first. Empty = drawn by the page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, TS)]
#[ts(export, export_to = "../../../packages/sdk/src/generated/")]
pub struct InstancePath(pub Vec<InstanceStep>);

/// Content Foliark wrote itself, so there is no input byte range. The reader never makes these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../../packages/sdk/src/generated/")]
pub enum GeneratedOrigin {
    Edit,
    Redaction,
}

/// Every item has exactly one origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export, export_to = "../../../packages/sdk/src/generated/")]
pub enum Origin {
    Source { loc: SourceLoc },
    Generated { origin: GeneratedOrigin },
}
