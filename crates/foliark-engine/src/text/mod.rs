//! Text extraction, selection and search over the semantic page model (spec §3).
//!
//! A [`TextLayout`] is one page's [`TextPage`] (always with clusters) plus the line each
//! cluster sits on, which selection needs. `extract_text` returns the wire `TextPage`.
use foliark_model::{ExtractOptions, PdfError, SearchHit, SearchOptions, TextPage};

mod extract;
mod search;
mod selection;

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

    /// Search every page (or `opts.pages`) in page order; a match never crosses a page.
    pub fn search(&self, query: &str, opts: &SearchOptions) -> Result<Vec<SearchHit>, PdfError> {
        let mut pages: Vec<usize> = match &opts.pages {
            Some(p) => p.iter().map(|&i| i as usize).collect(),
            None => (0..self.page_count()).collect(),
        };
        pages.sort_unstable();
        pages.dedup();
        let mut hits = Vec::new();
        for page in pages {
            let room = (opts.max_hits as usize).saturating_sub(hits.len());
            if room == 0 {
                break;
            }
            hits.extend(self.text_layout(page, false)?.search(query, opts, room));
        }
        Ok(hits)
    }
}
