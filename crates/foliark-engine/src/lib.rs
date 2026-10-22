//! Reader and renderer. Thin wrappers over hayro; the other §14 modules land with their tasks.
use foliark_model::{Limits, PageGeometry, PdfError};

pub mod edit;
pub mod interpret;
pub mod limits;
pub mod reader;
mod render;
pub mod text;
pub mod writer;

pub use render::{RenderOptions, RenderedPage};

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
}
