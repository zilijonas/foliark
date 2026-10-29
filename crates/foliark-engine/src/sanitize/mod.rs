//! Strict sanitization (spec §8). The allowlist writer drops every feature the spec removes by
//! construction (it never copies a key it does not list); this module reports what that
//! removal took out, so the export report is exact.
pub mod document;
pub mod interactive;
pub mod links;
pub mod prune;
pub mod resources;
pub mod text_semantics;
pub mod visibility;
