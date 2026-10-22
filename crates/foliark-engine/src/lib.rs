//! Reader and renderer. Thin wrappers over hayro; the other §14 modules land with their tasks.
use foliark_model::{Matrix, PageGeometry};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::page::Rotation;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use std::fmt;

pub mod limits;
pub mod reader;
use reader::ReaderError;

/// Spec §1.3: each rendered side is at most 16384 px.
const MAX_SIDE: f64 = 16384.0;

#[derive(Debug, PartialEq)]
pub enum EngineError {
    Read(ReaderError),
    PageOutOfRange { page: usize, count: usize },
    BadScale(f64),
    RenderTooLarge { width: f64, height: f64 },
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(e) => write!(f, "cannot read PDF: {e}"),
            Self::PageOutOfRange { page, count } => {
                write!(f, "page {page} out of range (count {count})")
            }
            Self::BadScale(s) => write!(f, "scale must be finite and > 0, got {s}"),
            Self::RenderTooLarge { width, height } => {
                write!(f, "render {width}x{height} exceeds {MAX_SIDE} px per side")
            }
        }
    }
}

impl std::error::Error for EngineError {}

impl From<ReaderError> for EngineError {
    fn from(e: ReaderError) -> Self {
        Self::Read(e)
    }
}

/// RGBA8 straight-alpha sRGB pixels, row-major (ImageData-compatible).
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

pub struct Document(reader::Document);

impl Document {
    pub fn open(bytes: Vec<u8>, password: Option<&str>) -> Result<Self, EngineError> {
        Self::open_with(bytes, password, false)
    }

    /// `allow_repaired`: accept a file whose xref table had to be rebuilt (spec §12 REPAIRED_PDF).
    pub fn open_with(
        bytes: Vec<u8>,
        password: Option<&str>,
        allow_repaired: bool,
    ) -> Result<Self, EngineError> {
        Ok(Self(reader::Document::open(
            bytes,
            password,
            allow_repaired,
        )?))
    }

    /// The strict reader behind this document.
    pub fn reader(&self) -> &reader::Document {
        &self.0
    }

    pub fn page_count(&self) -> usize {
        self.0.page_count()
    }

    pub fn page_geometry(&self, index: usize) -> Result<PageGeometry, EngineError> {
        let page = self.page(index)?;
        let (width, height) = page.render_dimensions();
        let rotation = match page.rotation() {
            Rotation::None => 0,
            Rotation::Horizontal => 90,
            Rotation::Flipped => 180,
            Rotation::FlippedHorizontal => 270,
        };
        let user_to_display = Matrix(page.initial_transform(true).as_coeffs());
        let (media, crop) = (page.media_box(), page.intersected_crop_box());
        Ok(PageGeometry {
            index: index as u32,
            width: width.into(),
            height: height.into(),
            rotation,
            // /UserUnit is not read yet (NEW-16); points == user units until then.
            user_unit: 1.0,
            media_box: [media.x0, media.y0, media.x1, media.y1],
            crop_box: [crop.x0, crop.y0, crop.x1, crop.y1],
            user_to_display,
            display_to_user: user_to_display
                .inverse()
                .expect("page transform is a rotation plus translation"),
        })
    }

    /// Render a page on an opaque white background at `scale` pixels per point.
    pub fn render_page(&self, index: usize, scale: f64) -> Result<Rendered, EngineError> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(EngineError::BadScale(scale));
        }
        let page = self.page(index)?;
        let (w, h) = page.render_dimensions();
        let (width, height) = (
            (f64::from(w) * scale).round().max(1.0),
            (f64::from(h) * scale).round().max(1.0),
        );
        if width > MAX_SIDE || height > MAX_SIDE {
            return Err(EngineError::RenderTooLarge { width, height });
        }
        // The renderer draws a page whose content fails to decode as blank; refuse instead.
        self.0.page_contents(index)?;
        let pixmap = render(
            page,
            &RenderCache::new(),
            &InterpreterSettings::default(),
            &RenderSettings::default(),
            &PixmapSettings {
                x_scale: scale as f32,
                y_scale: scale as f32,
                bg_color: WHITE,
            },
        );
        let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
        Ok(Rendered {
            width,
            height,
            data: pixmap.take_rgba8(ImageAlphaType::Alpha),
        })
    }

    fn page(&self, index: usize) -> Result<&hayro::hayro_syntax::page::Page<'_>, EngineError> {
        let pages = self.0.pdf().pages();
        pages.get(index).ok_or(EngineError::PageOutOfRange {
            page: index,
            count: pages.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_writer::{Content, Finish, Pdf as PdfOut, Rect, Ref};

    fn one_page(width: f32, height: f32) -> Vec<u8> {
        let (cat, tree, page, content) = (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4));
        let mut pdf = PdfOut::new();
        pdf.catalog(cat).pages(tree);
        pdf.pages(tree).kids([page]).count(1);
        let mut p = pdf.page(page);
        p.parent(tree)
            .media_box(Rect::new(0.0, 0.0, width, height))
            .contents(content);
        p.finish();
        let mut c = Content::new();
        c.set_fill_rgb(1.0, 0.0, 0.0)
            .rect(0.0, 0.0, width / 2.0, height / 2.0)
            .fill_nonzero();
        pdf.stream(content, &c.finish());
        pdf.finish()
    }

    #[test]
    fn renders_scaled_pixels_and_rejects_bad_requests() {
        let doc = Document::open(one_page(100.0, 50.0), None).unwrap();
        assert_eq!(doc.page_count(), 1);
        let r = doc.render_page(0, 2.0).unwrap();
        assert_eq!((r.width, r.height), (200, 100));
        // PDF origin is bottom-left: the red quarter sits in the bottom-left of the image.
        let px = |x: usize, y: usize| &r.data[(y * 200 + x) * 4..][..4];
        assert_eq!(px(10, 90), [255, 0, 0, 255]);
        assert_eq!(px(190, 10), [255, 255, 255, 255]);
        assert_eq!(
            doc.render_page(1, 1.0).err().unwrap(),
            EngineError::PageOutOfRange { page: 1, count: 1 }
        );
        assert_eq!(
            doc.render_page(0, 0.0).err().unwrap(),
            EngineError::BadScale(0.0)
        );
        assert!(matches!(
            doc.render_page(0, 1e6),
            Err(EngineError::RenderTooLarge { .. })
        ));
        assert!(matches!(
            Document::open(b"not a pdf".to_vec(), None),
            Err(EngineError::Read(_))
        ));
    }
}
