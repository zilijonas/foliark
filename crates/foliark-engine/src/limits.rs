//! Resource budgets (spec §9). Every check uses checked arithmetic and runs before the
//! allocation or work it guards; `LIMIT_EXCEEDED` carries which limit, the value and the max.
use foliark_model::{ErrorCode, Limits, ObjectId, PdfError};
use hayro::hayro_syntax::Filter;
use hayro::hayro_syntax::object::stream::DecodeFailure;
use hayro::hayro_syntax::object::{Dict, Name, Stream};
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

/// Most bytes an image XObject can decode to: its declared samples (rows padded to bytes, plus
/// one predictor byte per row) with the colour model's component count. Only the plain device
/// spaces are narrower than 4 components; any other space is bounded by 4. Dimensions are
/// already limited by `maxImagePixels`, so this stays bounded; an image is never held to the
/// generic per-stream cap, which a legitimate 81 MP RGB scan (243 MB) passes.
fn image_decoded_bound(d: &Dict<'_>, w: u32, h: u32) -> u64 {
    let mask = d.get::<bool>("ImageMask").unwrap_or(false);
    let bpc = if mask {
        1
    } else {
        u64::from(
            d.get::<u32>("BitsPerComponent")
                .or_else(|| d.get("BPC"))
                .unwrap_or(8),
        )
        .clamp(1, 16)
    };
    let comps = match d.get::<Name<'_>>("ColorSpace").or_else(|| d.get("CS")) {
        _ if mask => 1,
        Some(n) if matches!(n.as_str(), "DeviceGray" | "G" | "CalGray") => 1,
        Some(n) if matches!(n.as_str(), "DeviceRGB" | "RGB" | "CalRGB") => 3,
        _ => 4,
    };
    let row = (u64::from(w) * comps * bpc).div_ceil(8) + 1;
    row.saturating_mul(u64::from(h))
}

/// The decode cap of `stream`: (cap, the generic limit, whether the ratio set it). See `decode_stream`.
fn cap_of(stream: &Stream<'_>, limits: &Limits) -> Result<(u64, u64, bool), PdfError> {
    let mut max = u64::from(limits.max_stream_decoded_bytes);
    let ratio = u64::from(limits.max_decode_ratio);
    let allowed = (stream.raw_data().len() as u64)
        .saturating_mul(ratio)
        .max(MIB);
    let d = stream.dict();
    let image = d
        .get::<Name<'_>>("Subtype")
        .is_some_and(|n| n.as_str() == "Image");
    if image {
        let dim = |long, short| d.get::<u32>(long).or_else(|| d.get(short)).unwrap_or(0);
        let (w, h) = (dim("Width", "W"), dim("Height", "H"));
        // The dimension bomb is refused first; the bound below only widens a size-correct image.
        image_pixels(limits, w, h)?;
        max = max.max(image_decoded_bound(d, w, h));
    }
    let by_ratio = !image && allowed < max;
    Ok((if by_ratio { allowed } else { max }, max, by_ratio))
}

/// The budget scan's check of an image that only Flate encodes: the inflated size is counted in a
/// fixed buffer and refused past the cap, so a 243 MB scan is not held in memory just to be
/// measured (the predictor stage and the pixels are the reader's work, when the image is drawn).
/// `None` for any other filter chain: those go through `decode_stream`.
pub(crate) fn count_flate_image(
    stream: &Stream<'_>,
    limits: &Limits,
    budget: &mut Budget,
) -> Option<Result<(), PdfError>> {
    use std::io::Read;
    if stream.filters().as_slice() != [Filter::FlateDecode] {
        return None;
    }
    let (cap, max, _) = match cap_of(stream, limits) {
        Ok(c) => c,
        Err(e) => return Some(Err(e)),
    };
    let raw = stream.raw_data();
    let mut inflate = flate2::read::ZlibDecoder::new(&raw[..]);
    let (mut buf, mut n) = (vec![0u8; 64 * 1024], 0u64);
    // A stream zlib cannot read is `decode_stream`'s DecodeFailed, which the scan ignores.
    while let Ok(got) = inflate.read(&mut buf) {
        if got == 0 {
            return Some(budget.charge_bytes(usize::try_from(n).unwrap_or(usize::MAX)));
        }
        n += got as u64;
        if n > cap {
            return Some(Err(PdfError::limit("maxStreamDecodedBytes", cap + 1, max)));
        }
    }
    None
}

/// Strict decode of one stream under `maxStreamDecodedBytes` and `maxDecodeRatio`. An image
/// XObject is held to its declared size instead (`image_decoded_bound`, never below the generic
/// cap) and exempt from the ratio: deflate reaches about 1030:1 on flat data, so a blank
/// scanned page, 6.3 MB of white at 150 dpi, would otherwise look like a bomb. The ratio only
/// applies to other streams once the output passes 1 MiB. The decoder stops at the cap, so a
/// bomb is never built. `actual` is a lower bound: decoding stops at the first byte over the cap.
pub(crate) fn decode_stream<'a>(
    stream: &Stream<'a>,
    obj: ObjectId,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Cow<'a, [u8]>, PdfError> {
    let (cap, max, by_ratio) = cap_of(stream, limits)?;
    let ratio = u64::from(limits.max_decode_ratio);
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
