//! Paragraph editing (spec §6): selection, metrics and fixed surrounding-content budgets, and
//! applying an edit as removal of the selected text plus a planned layout drawn on top.
mod apply;
mod paragraph;
mod preview;

pub use apply::ParagraphApplier;
pub use paragraph::{Prepared, select_clusters};
pub use preview::layout_edit;
