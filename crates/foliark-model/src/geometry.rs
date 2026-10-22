//! Canonical page transforms (spec §2). `user` = PDF default user space of the page (before
//! UserUnit), `display` = points, top-left origin, y down, after CropBox, /Rotate and
//! /UserUnit; `pixel` = display scaled by the render scale.
use crate::{ErrorCode, Matrix, PageGeometry, PdfError, Point, Quad, Rect};

impl Matrix {
    pub const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn scale(sx: f64, sy: f64) -> Self {
        Self([sx, 0.0, 0.0, sy, 0.0, 0.0])
    }

    /// The matrix that applies `self` first and `next` second.
    pub fn then(self, next: Self) -> Self {
        let [a, b, c, d, e, f] = self.0;
        let [na, nb, nc, nd, ne, nf] = next.0;
        Self([
            a * na + b * nc,
            a * nb + b * nd,
            c * na + d * nc,
            c * nb + d * nd,
            e * na + f * nc + ne,
            e * nb + f * nd + nf,
        ])
    }

    pub fn apply(self, p: Point) -> Point {
        let [a, b, c, d, e, f] = self.0;
        Point {
            x: a * p.x + c * p.y + e,
            y: b * p.x + d * p.y + f,
        }
    }
}

impl Quad {
    /// Each corner is mapped, so the corner order (and with it the glyph orientation) is kept
    /// under any matrix, a mirroring one included.
    pub fn map(&self, m: Matrix) -> Self {
        Self(self.0.map(|p| m.apply(p)))
    }

    /// Axis-aligned bounds (the box of a non-axis-aligned quad, spec §2).
    pub fn bounds(&self) -> Rect {
        let xs = self.0.map(|p| p.x);
        let ys = self.0.map(|p| p.y);
        let (x0, x1) = (
            xs.into_iter().fold(f64::INFINITY, f64::min),
            xs.into_iter().fold(f64::NEG_INFINITY, f64::max),
        );
        let (y0, y1) = (
            ys.into_iter().fold(f64::INFINITY, f64::min),
            ys.into_iter().fold(f64::NEG_INFINITY, f64::max),
        );
        Rect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        }
    }
}

/// The geometry of one page (spec §2) from its raw entries. `media` and `crop` are user-space
/// boxes in any corner order (`crop` is `None` when the page has no CropBox); `rotate` is the
/// inherited /Rotate. Anything non-finite, a non-multiple of 90, a non-positive UserUnit or
/// an empty crop area is `MALFORMED_PDF`.
pub fn page_geometry(
    index: u32,
    media: [f64; 4],
    crop: Option<[f64; 4]>,
    rotate: i64,
    user_unit: f64,
) -> Result<PageGeometry, PdfError> {
    let malformed = || PdfError::new(ErrorCode::MalformedPdf);
    let norm = |b: [f64; 4]| {
        b.iter().all(|v| v.is_finite()).then(|| {
            [
                b[0].min(b[2]) + 0.0,
                b[1].min(b[3]) + 0.0,
                b[0].max(b[2]) + 0.0,
                b[1].max(b[3]) + 0.0,
            ]
        })
    };
    let media = norm(media).ok_or_else(malformed)?;
    let crop = match crop {
        Some(c) => {
            let c = norm(c).ok_or_else(malformed)?;
            [
                c[0].max(media[0]),
                c[1].max(media[1]),
                c[2].min(media[2]),
                c[3].min(media[3]),
            ]
        }
        None => media,
    };
    let (w0, h0) = (crop[2] - crop[0], crop[3] - crop[1]);
    if !(w0 > 0.0 && h0 > 0.0 && user_unit.is_finite() && user_unit > 0.0) {
        return Err(malformed());
    }
    let (u, [cx0, cy0, cx1, cy1]) = (user_unit, crop);
    let (rotation, m) = match rotate.rem_euclid(360) {
        0 => (0, [u, 0.0, 0.0, -u, -u * cx0, u * cy1]),
        90 => (90, [0.0, u, u, 0.0, -u * cy0, -u * cx0]),
        180 => (180, [-u, 0.0, 0.0, u, u * cx1, -u * cy0]),
        270 => (270, [0.0, -u, -u, 0.0, u * cy1, u * cx1]),
        _ => return Err(malformed()),
    };
    let (width, height) = if rotation % 180 == 0 {
        (u * w0, u * h0)
    } else {
        (u * h0, u * w0)
    };
    let user_to_display = Matrix(m.map(|v| v + 0.0));
    let display_to_user = user_to_display.inverse().ok_or_else(malformed)?;
    // Scaling by a tiny UserUnit can underflow a positive box to zero.
    let finite = [width, height]
        .iter()
        .chain(&user_to_display.0)
        .chain(&display_to_user.0)
        .all(|v| v.is_finite());
    if !(finite && width > 0.0 && height > 0.0) {
        return Err(malformed());
    }
    Ok(PageGeometry {
        index,
        width,
        height,
        rotation,
        user_unit,
        media_box: media,
        crop_box: crop,
        user_to_display,
        display_to_user,
    })
}

impl PageGeometry {
    /// User space to pixels of a render at `scale` pixels per display point.
    pub fn user_to_pixel(&self, scale: f64) -> Matrix {
        self.user_to_display.then(Matrix::scale(scale, scale))
    }

    /// Pixels of a render at `scale` back to user space.
    pub fn pixel_to_user(&self, scale: f64) -> Option<Matrix> {
        Matrix::scale(scale, scale)
            .inverse()
            .map(|m| m.then(self.display_to_user))
    }

    /// HDDN's normalized box: fractions of the display size, clamped to the page (spec §2).
    pub fn normalize(&self, r: Rect) -> Rect {
        let clamp = |v: f64| v.clamp(0.0, 1.0);
        let (nx, ny) = (clamp(r.x / self.width), clamp(r.y / self.height));
        Rect {
            x: nx,
            y: ny,
            width: clamp((r.x + r.width) / self.width) - nx,
            height: clamp((r.y + r.height) / self.height) - ny,
        }
    }

    pub fn denormalize(&self, n: Rect) -> Rect {
        Rect {
            x: n.x * self.width,
            y: n.y * self.height,
            width: n.width * self.width,
            height: n.height * self.height,
        }
    }
}
