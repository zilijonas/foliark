//! The per-document text index (spec §4): page layouts built lazily from the working copy and
//! bound to the revision they were built for, so no result carries a revision it was not
//! computed at. After a commit the index either drops everything (the working copy was
//! replaced, so every glyph id may have moved) or keeps the layouts (only redactions changed,
//! and those never alter text) and restamps them.
use super::extract::TextLayout;
use crate::Document;
use crate::cache::Lru;
use foliark_model::{Cluster, ErrorCode, PdfError, StyleRun, Word};
use std::mem::size_of;

type Key = (u32, bool);

pub(crate) struct TextIndex {
    doc_id: String,
    revision: u32,
    layouts: Lru<Key, TextLayout>,
}

/// An estimate of the heap a layout holds: its text, words, styles, clusters and per-cluster meta.
fn layout_bytes(l: &TextLayout) -> usize {
    let p = &l.page;
    p.text.len()
        + p.words
            .iter()
            .map(|w| size_of::<Word>() + w.text.len())
            .sum::<usize>()
        + p.styles.len() * size_of::<StyleRun>()
        + p.clusters.as_ref().map_or(0, Vec::len) * size_of::<Cluster>()
        + l.meta_bytes()
}

impl TextIndex {
    pub(crate) fn new(doc_id: String, cap: usize) -> Self {
        Self {
            doc_id,
            revision: 0,
            layouts: Lru::new(cap),
        }
    }

    /// The layout of `page` of `doc` (the working copy at this index's revision), built on first use.
    pub(crate) fn layout(
        &mut self,
        doc: &Document,
        page: u32,
        hidden: bool,
    ) -> Result<&TextLayout, PdfError> {
        let key = (page, hidden);
        if !self.layouts.contains(&key) {
            let mut l = doc.text_layout(page as usize, hidden)?;
            l.page.doc_id.clone_from(&self.doc_id);
            l.page.revision = self.revision;
            let bytes = layout_bytes(&l);
            self.layouts.insert(key, l, bytes);
        }
        self.layouts
            .get(&key)
            .ok_or(PdfError::new(ErrorCode::Internal))
    }

    /// A commit moved the document to `revision`. `replaced`: the working copy is a new file.
    pub(crate) fn advance(&mut self, revision: u32, replaced: bool) {
        if replaced {
            self.layouts.clear();
        } else {
            for l in self.layouts.values_mut() {
                l.page.revision = revision;
            }
        }
        self.revision = revision;
    }

    pub(crate) fn bytes(&self) -> usize {
        self.layouts.bytes()
    }

    pub(crate) fn len(&self) -> usize {
        self.layouts.len()
    }

    pub(crate) fn evictions(&self) -> u64 {
        self.layouts.evictions()
    }
}
