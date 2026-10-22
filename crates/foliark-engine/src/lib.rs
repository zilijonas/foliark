//! Reader and renderer. Thin wrappers over hayro; the other §14 modules land with their tasks.
use foliark_model::{ErrorCode, Limits, PageGeometry, PdfError};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};

pub mod interpret;
pub mod limits;
pub mod reader;
pub mod writer;

/// Spec §1.3: each rendered side is at most 16384 px.
const MAX_SIDE: f64 = 16384.0;

/// RGBA8 straight-alpha sRGB pixels, row-major (ImageData-compatible).
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

pub struct Document(reader::Document);

impl Document {
    pub fn open(bytes: Vec<u8>, password: Option<&str>) -> Result<Self, PdfError> {
        Self::open_with(bytes, password, false)
    }

    /// `allow_repaired`: accept a file whose xref table had to be rebuilt (spec §12 REPAIRED_PDF).
    pub fn open_with(
        bytes: Vec<u8>,
        password: Option<&str>,
        allow_repaired: bool,
    ) -> Result<Self, PdfError> {
        Self::open_with_limits(bytes, password, allow_repaired, Limits::default())
    }

    pub fn open_with_limits(
        bytes: Vec<u8>,
        password: Option<&str>,
        allow_repaired: bool,
        limits: Limits,
    ) -> Result<Self, PdfError> {
        Ok(Self(reader::Document::open_with_limits(
            bytes,
            password,
            allow_repaired,
            limits,
        )?))
    }

    /// The strict reader behind this document.
    pub fn reader(&self) -> &reader::Document {
        &self.0
    }

    pub fn page_count(&self) -> usize {
        self.0.page_count()
    }

    pub fn page_model(&self, index: usize) -> Result<interpret::SemanticPage, PdfError> {
        self.0.page_model(index)
    }

    pub fn page_geometry(&self, index: usize) -> Result<PageGeometry, PdfError> {
        self.0.page_geometry(index)
    }

    /// Render a page on an opaque white background at `scale` pixels per point.
    pub fn render_page(&self, index: usize, scale: f64) -> Result<Rendered, PdfError> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(PdfError::new(ErrorCode::InvalidArgument));
        }
        let page = self.page(index)?;
        let (w, h) = page.render_dimensions();
        let (width, height) = (
            (f64::from(w) * scale).round().max(1.0),
            (f64::from(h) * scale).round().max(1.0),
        );
        if width > MAX_SIDE || height > MAX_SIDE {
            return Err(PdfError::limit(
                "maxRenderSide",
                width.max(height) as u64,
                MAX_SIDE as u64,
            ));
        }
        let max = u64::from(self.0.limits().max_render_pixels);
        limits::check("maxRenderPixels", (width * height) as u64, max)?;
        // The renderer decodes fonts and images without caps: refuse bombs first, and refuse a
        // page whose content fails to decode instead of drawing it blank.
        self.0.ensure_budgets()?;
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

    fn page(&self, index: usize) -> Result<&hayro::hayro_syntax::page::Page<'_>, PdfError> {
        let pages = self.0.pdf().pages();
        pages.get(index).ok_or(PdfError::page_out_of_range(index))
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
            PdfError::page_out_of_range(1)
        );
        assert_eq!(
            doc.render_page(0, 0.0).err().unwrap().code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            doc.render_page(0, 1e6).err().unwrap().code,
            ErrorCode::LimitExceeded
        );
        assert_eq!(
            Document::open(b"not a pdf".to_vec(), None)
                .err()
                .unwrap()
                .code,
            ErrorCode::MalformedPdf
        );
    }
}
