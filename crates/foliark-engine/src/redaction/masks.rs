//! Alpha and stencil planes of an erased image (spec §5.2): soft masks (`SMask`, an explicit
//! `Mask` stream, a color-key `Mask` array, `SMaskInData`) and `ImageMask` stencils.
//!
//! An image with alpha is re-encoded with ONE 8-bit DeviceGray `SMask` of the grid hayro
//! decoded (the mask's own resolution, which may differ from the image's); every sample whose
//! pixel square meets a region is set opaque, so transparency cannot hide what the color plane
//! used to hold, and color-key arrays and explicit masks are folded into it (the key would
//! otherwise match the black the erased samples now hold). A stencil is re-encoded as a 1-bit
//! `ImageMask` with the erased samples unpainted. The source mask objects are not referenced
//! by the replacement, so the writer leaves them out of the file.
use super::image::{Encoded, Plane, erase_grid, image_dict, precheck};
use crate::interpret::ImageItem;
use crate::limits::Budget;
use crate::writer::Val;
use foliark_model::{ErrorCode, Limits, ObjectId, PdfError};
use hayro::hayro_interpret::StencilImage;
use hayro::hayro_syntax::object::Stream;
use hayro::vello_cpu::peniko::kurbo::Affine;

/// The soft mask of an erased image: `a` with the samples under `rects` opaque.
pub(super) fn erase_alpha(
    mut a: Plane,
    m: Affine,
    rects: &[[f64; 4]],
) -> Result<Encoded, PdfError> {
    erase_grid(a.w, a.h, m, rects, |s| a.data[s] = 255)?;
    Ok(Encoded {
        dict: image_dict(a.w, a.h, "DeviceGray", false),
        data: a.data,
        smask: None,
    })
}

/// A stencil mask with the samples under `rects` unpainted, as a 1-bit `ImageMask`.
pub(super) fn encode_stencil(
    stream: &Stream<'_>,
    item: &ImageItem,
    id: ObjectId,
    m: Affine,
    rects: &[[f64; 4]],
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Encoded, PdfError> {
    let failed = || PdfError::at(ErrorCode::DecodeFailed, id);
    precheck(stream, item, id, limits, budget)?;
    let stencil = StencilImage::from_stream(stream).ok_or_else(failed)?;
    let mut out = None;
    stencil.with_stencil(|luma, _| out = Some(luma), None);
    let luma = out.ok_or_else(failed)?;
    if luma.scale_factors != (1.0, 1.0) {
        return Err(failed());
    }
    let interpolate = luma.interpolate;
    let mut plane = Plane {
        w: luma.width,
        h: luma.height,
        data: luma.data,
    }
    .check(1, id)?;
    if (plane.w, plane.h) != (item.width, item.height) {
        return Err(failed());
    }
    // Hayro: 255 paints. PDF stencil: sample 0 paints (default /Decode [0 1]).
    erase_grid(plane.w, plane.h, m, rects, |s| plane.data[s] = 0)?;
    let row = (plane.w as usize).div_ceil(8);
    let mut bits = vec![0u8; row * plane.h as usize];
    for (j, line) in plane.data.chunks(plane.w as usize).enumerate() {
        for (i, v) in line.iter().enumerate() {
            if *v < 128 {
                bits[j * row + i / 8] |= 0x80 >> (i % 8);
            }
        }
    }
    let mut dict = image_dict(plane.w, plane.h, "DeviceGray", interpolate);
    dict.retain(|(k, _)| k != "ColorSpace" && k != "BitsPerComponent");
    dict.push(("BitsPerComponent".into(), Val::Int(1)));
    dict.push(("ImageMask".into(), Val::Bool(true)));
    Ok(Encoded {
        dict,
        data: bits,
        smask: None,
    })
}
