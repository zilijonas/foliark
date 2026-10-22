//! Downscaling of decoded image samples with the renderer's own filter (Foliark patch P17).
//!
//! The renderer asks for an image at the size it will be drawn; handing it that size directly
//! replaces its general-purpose resampler, which dominates the cost of a scanned page. The filter
//! is the one the renderer uses (Hermite, see `hayro::image::RESAMPLING_FUNCTION`), so the pixels stay
//! close to what it would have produced from the full-size image.
use crate::{ImageData, LumaData, RgbData};

/// Images only shrink by at least this factor on an axis; a smaller step keeps that axis as is.
const MIN_RATIO: u64 = 6; // over 5, i.e. 1.2

/// The size to reduce a `w` x `h` image to for a device target of `tw` x `th` pixels (the drawn size,
/// rounded up, which is also the size the renderer would resample to), or `None` if it should stay
/// as it is.
pub(crate) fn reduced_size(w: u32, h: u32, (tw, th): (u32, u32)) -> Option<(u32, u32)> {
    let axis = |src: u32, target: u32| {
        let t = target.max(1);
        if u64::from(src) * 5 >= u64::from(t) * MIN_RATIO {
            t
        } else {
            src
        }
    };
    let size = (axis(w, tw), axis(h, th));
    (size != (w, h)).then_some(size)
}

/// Reduce decoded samples to `size` (from [`reduced_size`]); the scale factors grow so the image
/// still covers the same area.
pub(crate) fn reduce(image: ImageData, size: (u32, u32)) -> ImageData {
    match image {
        ImageData::Luma(d) => {
            let (data, factors) =
                resample::<1>(&d.data, (d.width, d.height), size, d.scale_factors);
            ImageData::Luma(LumaData {
                data,
                width: size.0,
                height: size.1,
                scale_factors: factors,
                ..d
            })
        }
        ImageData::Rgb(d) => {
            let (data, factors) =
                resample::<3>(&d.data, (d.width, d.height), size, d.scale_factors);
            ImageData::Rgb(RgbData {
                data,
                width: size.0,
                height: size.1,
                scale_factors: factors,
                ..d
            })
        }
    }
}

/// Hermite (smoothstep) kernel, support 1.
fn hermite(x: f64) -> f64 {
    let x = x.abs();
    if x < 1.0 {
        (2.0 * x - 3.0) * x * x + 1.0
    } else {
        0.0
    }
}

/// Taps for shrinking `src` pixels to `out`, stored tap-major so one tap of every output pixel is
/// a contiguous run: the kernel is stretched by the ratio and centred on each output pixel, edges
/// are clamped (the weights are renormalized instead). Every output pixel has `len` taps from
/// `starts[i]`; missing ones weigh 0.
struct Taps {
    starts: Vec<usize>,
    len: usize,
    weights: Vec<f32>,
}

impl Taps {
    fn new(src: usize, out: usize) -> Self {
        let k = src as f64 / out as f64;
        let spans: Vec<(usize, usize)> = (0..out)
            .map(|i| {
                let c = (i as f64 + 0.5) * k;
                (
                    ((c - k).floor() as i64).max(0) as usize,
                    ((c + k).ceil() as usize).min(src),
                )
            })
            .collect();
        let len = spans.iter().map(|(a, b)| b - a).max().unwrap_or(1).min(src);
        let mut weights = vec![0f32; len * out];
        let mut starts = Vec::with_capacity(out);
        for (i, &(first, last)) in spans.iter().enumerate() {
            let start = first.min(src - len);
            let c = (i as f64 + 0.5) * k;
            let raw: Vec<f64> = (first..last)
                .map(|p| hermite((p as f64 + 0.5 - c) / k))
                .collect();
            let sum: f64 = raw.iter().sum();
            for (p, w) in (first..last).zip(raw) {
                weights[(p - start) * out + i] = (w / sum) as f32;
            }
            starts.push(start);
        }
        Self {
            starts,
            len,
            weights,
        }
    }
}

/// Separable resample: vertical first (whole rows into one accumulator), then horizontal on each
/// reduced row. Both loops accumulate over independent elements so the compiler can vectorize or
/// at least pipeline them.
fn resample<const N: usize>(
    src: &[u8],
    (w, h): (u32, u32),
    (rw, rh): (u32, u32),
    (sx, sy): (f32, f32),
) -> (Vec<u8>, (f32, f32)) {
    let (w, h, rw, rh) = (w as usize, h as usize, rw as usize, rh as usize);
    let (cols, rows) = (Taps::new(w, rw), Taps::new(h, rh));
    let row_len = w * N;
    let mut acc = vec![0f32; row_len];
    let mut line = vec![0f32; rw * N];
    let mut out = Vec::with_capacity(rw * rh * N);
    for (y, &start) in rows.starts.iter().enumerate() {
        acc.fill(0.0);
        for t in 0..rows.len {
            let weight = rows.weights[t * rh + y];
            if weight == 0.0 {
                continue;
            }
            let r = start + t;
            for (a, &s) in acc.iter_mut().zip(&src[r * row_len..(r + 1) * row_len]) {
                *a += f32::from(s) * weight;
            }
        }
        line.fill(0.0);
        for t in 0..cols.len {
            let weights = &cols.weights[t * rw..(t + 1) * rw];
            for (x, (&weight, &start)) in weights.iter().zip(&cols.starts).enumerate() {
                let from = (start + t) * N;
                for ch in 0..N {
                    line[x * N + ch] += weight * acc[from + ch];
                }
            }
        }
        out.extend(line.iter().map(|v| (v + 0.5) as u8));
    }
    (out, (sx * w as f32 / rw as f32, sy * h as f32 / rh as f32))
}
