//! Text extraction, selection and search over the semantic page model (spec §3).
//!
//! A [`TextLayout`] is one page's [`TextPage`] (always with clusters) plus the line each
//! cluster sits on, which selection needs. `extract_text` returns the wire `TextPage`.
use foliark_model::{ExtractOptions, PdfError, TextPage};

mod extract;

pub use extract::TextLayout;

impl crate::Document {
    /// The extracted text of one page (spec §3.4). `doc_id` is empty and `revision` is 0 here:
    /// the session layer that owns the document identity and the revision fills them.
    pub fn extract_text(&self, page: usize, opts: &ExtractOptions) -> Result<TextPage, PdfError> {
        let mut text = self.text_layout(page, opts.include_ocg_hidden)?.page;
        if !opts.chars {
            text.clusters = None;
        }
        Ok(text)
    }

    /// A page's text with cluster geometry, for hit testing, selection and search.
    pub fn text_layout(
        &self,
        page: usize,
        include_ocg_hidden: bool,
    ) -> Result<TextLayout, PdfError> {
        TextLayout::build(&self.0.page_model(page)?, include_ocg_hidden)
    }
}
