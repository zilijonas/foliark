//! Wire DTOs shared by the engine, the wasm facade and the generated TypeScript types.
//! Only what the render path needs so far; the rest of spec §1 lands with its owning task.
use serde::Serialize;
use ts_rs::TS;

/// Display-space size of a page (spec §2: points, after CropBox and /Rotate).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../../packages/sdk/src/generated/")]
pub struct PageGeometry {
    pub index: u32,
    pub width: f64,
    pub height: f64,
    /// Clockwise /Rotate in degrees: 0, 90, 180 or 270.
    pub rotation: u16,
}
