//! Resource budgets (spec §9). Every check uses checked arithmetic and runs before the
//! allocation or work it guards; `LIMIT_EXCEEDED` carries which limit, the value and the max.
use foliark_model::{ErrorCode, Limits, ObjectId, PdfError};
use hayro::hayro_syntax::object::stream::DecodeFailure;
use hayro::hayro_syntax::object::{Name, Stream};
use std::borrow::Cow;

const MIB: u64 = 1 << 20;

pub(crate) fn check(limit: &'static str, actual: u64, max: u64) -> Result<(), PdfError> {
    if actual > max {
        return Err(PdfError::limit(limit, actual, max));
    }
    Ok(())
}

/// Spec §9 `maxOutputBytes` covers the writer buffer and the transfer copy: an output of `len`
/// bytes is held once by the engine and handed to JS once more, so it meters twice. Checked
/// before the bytes are built where their size is known, and again on the finished buffer.
/// `handed_over` is false for a working copy, which never leaves the engine (meters once).
pub(crate) fn check_output(len: u64, handed_over: bool, limits: &Limits) -> Result<(), PdfError> {
    check(
        "maxOutputBytes",
        len.saturating_mul(1 + u64::from(handed_over)),
        limits.max_output_bytes.into(),
    )
}

/// Work units of one operation (spec §9): one interpreted operator, 1 KiB decoded or encoded,
/// 1,000 pixels, or one shaped cluster. Decoded bytes of all streams add up here, so there is
/// no separate total-decoded limit.
pub struct Budget {
    used: u64,
    max: u64,
}

impl Budget {
    pub fn new(limits: &Limits) -> Self {
        Self {
            used: 0,
            max: limits.max_work_units,
        }
    }

    pub fn charge(&mut self, units: u64) -> Result<(), PdfError> {
        self.used = self.used.saturating_add(units);
        check("maxWorkUnits", self.used, self.max)
    }

    pub fn charge_bytes(&mut self, bytes: usize) -> Result<(), PdfError> {
        self.charge((bytes as u64).div_ceil(1024))
    }

    pub fn charge_pixels(&mut self, pixels: u64) -> Result<(), PdfError> {
        self.charge(pixels.div_ceil(1000))
    }
}

/// An image of `width` x `height` (dimension bomb check). Returns the pixel count.
pub(crate) fn image_pixels(limits: &Limits, width: u32, height: u32) -> Result<u64, PdfError> {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    check("maxImagePixels", pixels, limits.max_image_pixels.into())?;
    Ok(pixels)
}

/// Strict decode of one stream under `maxStreamDecodedBytes` and `maxDecodeRatio` (the ratio
/// only applies once the output passes 1 MiB, and never to an image XObject: its decoded size
/// is bounded by `maxImagePixels` and deflate reaches about 1030:1 on flat data, so a blank
/// scanned page, 6.3 MB of white at 150 dpi, would otherwise look like a bomb). The decoder stops at the cap, so a bomb is
/// never built. `actual` is a lower bound: decoding stops at the first byte over the cap.
pub(crate) fn decode_stream<'a>(
    stream: &Stream<'a>,
    obj: ObjectId,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Cow<'a, [u8]>, PdfError> {
    let max = u64::from(limits.max_stream_decoded_bytes);
    let ratio = u64::from(limits.max_decode_ratio);
    let allowed = (stream.raw_data().len() as u64)
        .saturating_mul(ratio)
        .max(MIB);
    let image = stream
        .dict()
        .get::<Name<'_>>("Subtype")
        .is_some_and(|n| n.as_str() == "Image");
    let by_ratio = !image && allowed < max;
    let cap = if by_ratio { allowed } else { max };
    let cap_usize = usize::try_from(cap).unwrap_or(usize::MAX);
    match stream.decoded_capped(cap_usize) {
        Ok(data) => {
            budget.charge_bytes(data.len())?;
            Ok(data)
        }
        Err(DecodeFailure::TooLarge) if by_ratio => {
            let raw = stream.raw_data().len().max(1) as u64;
            Err(PdfError::limit(
                "maxDecodeRatio",
                (cap + 1).div_ceil(raw),
                ratio,
            ))
        }
        Err(DecodeFailure::TooLarge) => Err(PdfError::limit("maxStreamDecodedBytes", cap + 1, max)),
        Err(_) => Err(PdfError::at(ErrorCode::DecodeFailed, obj)),
    }
}
