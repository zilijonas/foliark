//! Raster pixels (spec §7; NEW-55): one page of the working copy drawn at the policy scale, then
//! every region burned into the buffer. The page is drawn from the working copy the caller
//! passes (edits applied), never from an earlier preview, and exactly one page buffer is alive
//! at a time: the job encodes it and drops it before the next page.
use super::raster_plan::{Burn, PageSpec, RasterPlan};
use crate::{Document, RenderOptions};
use foliark_model::PdfError;

/// A drawn page: RGBA8 (opaque), `width * height * 4` bytes, row-major.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Spec §1.3 `complete`: no render warning.
    pub complete: bool,
}

/// Set every pixel whose square intersects a burn rectangle to its fill. Pixel `(i, j)` covers
/// the display rect `[i/scale, (i+1)/scale] x [j/scale, (j+1)/scale]` (spec §1.3), so the burned
/// range is `floor(x0*scale) .. ceil(x1*scale)` per axis: a square that only touches the region
/// along an edge is left alone, one that the region merely grazes is erased whole.
pub fn burn(rgba: &mut [u8], width: u32, height: u32, scale: f64, burns: &[Burn]) {
    let span = |lo: f64, len: f64, max: u32| {
        let a = (lo * scale).floor().clamp(0.0, f64::from(max));
        let b = ((lo + len) * scale).ceil().clamp(0.0, f64::from(max));
        (a as usize, b as usize)
    };
    for b in burns {
        let (x0, x1) = span(b.rect.x, b.rect.width, width);
        let (y0, y1) = span(b.rect.y, b.rect.height, height);
        for y in y0..y1 {
            let row = &mut rgba[(y * width as usize + x0) * 4..(y * width as usize + x1) * 4];
            for px in row.chunks_exact_mut(4) {
                px[..3].copy_from_slice(&b.fill);
                px[3] = 255;
            }
        }
    }
}

/// Draw page `index` and burn its regions. `render_page` enforces the render limits again and
/// stops on a cancel; the plan already refused pages over them.
pub fn render(
    doc: &Document,
    plan: &RasterPlan,
    index: usize,
    spec: &PageSpec,
) -> Result<Pixels, PdfError> {
    let page = doc.render_page(
        index,
        RenderOptions {
            annotations: plan.annotations,
            ..RenderOptions::at(plan.scale)
        },
    )?;
    let (width, height) = (page.header.width, page.header.height);
    debug_assert_eq!((width, height), (spec.width_px, spec.height_px));
    let mut rgba = page.data;
    burn(&mut rgba, width, height, plan.scale, &plan.burns[index]);
    Ok(Pixels {
        width,
        height,
        rgba,
        complete: page.header.complete,
    })
}
