//! Text options and the §3.3 grouping constants.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Spec §3.3. `em` is the larger font size of the two neighbours, in display points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grouping {
    /// Max perpendicular distance of a glyph origin from the line's baseline, in em.
    pub same_line_perp: f64,
    /// Max overlap with the previous glyph's advance end that still continues the line, in em.
    pub new_line_back: f64,
    /// Max gap after the previous advance end that still continues the line, in em.
    pub new_line_fwd: f64,
    /// A gap of at least this many em breaks a word.
    pub word_gap: f64,
    /// Max baseline advance, in em, for a line to join the previous line's block.
    pub block_gap: f64,
}

pub const GROUPING: Grouping = Grouping {
    same_line_perp: 0.5,
    new_line_back: 0.5,
    new_line_fwd: 5.0,
    word_gap: 0.2,
    block_gap: 1.5,
};

/// `extractText` options (spec §1): `chars` adds `TextPage::clusters`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export, optional_fields)]
pub struct ExtractOptions {
    #[ts(type = "boolean", optional)]
    pub chars: bool,
    #[ts(type = "boolean", optional)]
    pub include_ocg_hidden: bool,
}

/// `search` options (spec §1.2). Defaults: case-insensitive, diacritic-sensitive, 1000 hits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export, optional_fields)]
pub struct SearchOptions {
    pub pages: Option<Vec<u32>>,
    #[ts(type = "boolean", optional)]
    pub case_sensitive: bool,
    #[ts(type = "boolean", optional)]
    pub diacritic_sensitive: bool,
    #[ts(type = "boolean", optional)]
    pub whole_word: bool,
    #[ts(type = "number", optional)]
    pub max_hits: u32,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            pages: None,
            case_sensitive: false,
            diacritic_sensitive: true,
            whole_word: false,
            max_hits: 1000,
        }
    }
}
