//! Per-document state and the `applyChanges` transaction (spec §4).
//!
//! A session owns the opened source, the revision, the committed change intent (the edit list
//! and the redaction list) and the working copy that the edit list produces. A `ChangeSet` is
//! validated whole, then committed whole: any rejection leaves every field untouched.
//!
//! Nothing here exports. Redactions are stored as intent (regions plus must-remove clusters)
//! and run at export (spec §5); text edits are applied by an [`EditApplier`] (NEW-51).
use crate::Document;
use crate::engine::not_built;
use crate::text::{ClusterId, TextIndex, TextLayout};
use foliark_model::{
    ChangeResult, ChangeSet, DocumentInfo, EditLayout, ErrorCode, Limits, PdfError, Rect,
    Redaction, RejectedChange, TextEdit,
};
use std::collections::{BTreeSet, HashSet};

/// What a plan or cache must match to still be valid (spec §4): the same document in the same
/// worker generation at the same revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub doc_id: String,
    pub generation: u32,
    pub revision: u32,
}

/// A committed redaction. A `text` redaction is stored resolved: one display-space region per
/// visual line plus the clusters it must remove (a cluster the range cut counts whole). A
/// `region` redaction has one region and no must-remove set. The cluster ids name glyphs of
/// the working copy that was current at the base revision.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredRedaction {
    pub id: String,
    pub page_index: u32,
    pub regions: Vec<Rect>,
    /// `None` = black.
    pub fill: Option<[f64; 3]>,
    pub must_remove: Vec<ClusterId>,
}

/// The committed intent of a revision. Both lists are exactly what the last accepted
/// `ChangeSet` left (or inherited), so undo is a resubmission of an older list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Committed {
    pub edits: Vec<TextEdit>,
    pub redactions: Vec<StoredRedaction>,
}

impl Committed {
    /// The ids of every committed operation.
    pub fn op_ids(&self) -> impl Iterator<Item = &str> {
        let edits = self.edits.iter().map(|e| e.id.as_str());
        edits.chain(self.redactions.iter().map(|r| r.id.as_str()))
    }
}

/// A working copy built from the source and an edit list.
pub struct Applied {
    /// A complete PDF the strict reader reopens, with the source's page count.
    pub bytes: Vec<u8>,
    /// One per edit that was laid out.
    pub layouts: Vec<EditLayout>,
}

/// The seam NEW-51 fills: replay `edits` from `source` in array order (each edit's rects are
/// evaluated on the state after the previous ones) and return the new working copy, or one
/// rejection per edit that cannot apply. Called only with a non-empty list whose edits passed
/// structural validation. It must not keep state: a rejection or a later failure discards
/// whatever it produced.
pub trait EditApplier {
    fn apply(
        &self,
        source: &[u8],
        edits: &[TextEdit],
        limits: &Limits,
    ) -> Result<Applied, Vec<RejectedChange>>;
}

/// Text edits are not built yet: every edit is rejected `INVALID_ARGUMENT` with
/// `details.feature = "textEdits"` (the convention of the other unbuilt ops).
pub struct NotBuilt;

impl EditApplier for NotBuilt {
    fn apply(
        &self,
        _: &[u8],
        edits: &[TextEdit],
        _: &Limits,
    ) -> Result<Applied, Vec<RejectedChange>> {
        Err(edits
            .iter()
            .map(|e| reject(&e.id, not_built("textEdits")))
            .collect())
    }
}

fn reject(id: &str, e: PdfError) -> RejectedChange {
    RejectedChange {
        id: id.to_owned(),
        code: e.code,
        details: e.details.map(|d| *d),
    }
}

fn invalid() -> PdfError {
    PdfError::new(ErrorCode::InvalidArgument)
}

fn rect_ok(r: &Rect) -> bool {
    [r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite()) && r.width > 0.0 && r.height > 0.0
}

fn fill_ok(fill: Option<[f64; 3]>) -> bool {
    fill.is_none_or(|f| f.iter().all(|c| (0.0..=1.0).contains(c)))
}

pub(crate) struct Session {
    source: Document,
    source_bytes: Vec<u8>,
    /// Present iff the committed edit list is not empty; otherwise the source is the working copy.
    working: Option<Document>,
    pub(crate) info: DocumentInfo,
    pub(crate) revision: u32,
    pub(crate) committed: Committed,
    /// Lazily built page layouts, byte-bounded and bound to `revision` (spec §4).
    pub(crate) index: TextIndex,
}

impl Session {
    pub(crate) fn new(
        id: String,
        source_bytes: Vec<u8>,
        source: Document,
        info: DocumentInfo,
        cache_bytes: usize,
    ) -> Self {
        Self {
            index: TextIndex::new(id, cache_bytes),
            source,
            source_bytes,
            working: None,
            info,
            revision: 0,
            committed: Committed::default(),
        }
    }

    /// Bytes of the input this session keeps (the source is held a second time, parsed).
    pub(crate) fn source_len(&self) -> usize {
        self.source_bytes.len()
    }

    /// The working copy at the current revision: what extract, render, select and search read.
    pub(crate) fn doc(&self) -> &Document {
        self.working.as_ref().unwrap_or(&self.source)
    }

    pub(crate) fn layout(&mut self, page: u32, hidden: bool) -> Result<&TextLayout, PdfError> {
        let doc = self.working.as_ref().unwrap_or(&self.source);
        self.index.layout(doc, page, hidden)
    }

    fn check_page(&self, page: u32) -> Result<(), PdfError> {
        if (page as usize) < self.doc().page_count() {
            Ok(())
        } else {
            Err(PdfError::page_out_of_range(page as usize))
        }
    }

    fn check_edit(&self, e: &TextEdit) -> Result<(), PdfError> {
        self.check_page(e.page_index)?;
        if e.rects.is_empty() || !e.rects.iter().all(rect_ok) {
            return Err(invalid());
        }
        Ok(())
    }

    /// Text ranges resolve against this session's current text, which is the base revision's
    /// (a stale base never gets here). The page text is the default one (no OCG-hidden content).
    fn resolve(&mut self, r: &Redaction) -> Result<StoredRedaction, PdfError> {
        match r {
            Redaction::Region {
                id,
                page_index,
                rect,
                fill,
            } => {
                self.check_page(*page_index)?;
                if !rect_ok(rect) || !fill_ok(*fill) {
                    return Err(invalid());
                }
                Ok(StoredRedaction {
                    id: id.clone(),
                    page_index: *page_index,
                    regions: vec![*rect],
                    fill: *fill,
                    must_remove: Vec::new(),
                })
            }
            Redaction::Text {
                id,
                page_index,
                start,
                end,
                fill,
            } => {
                self.check_page(*page_index)?;
                if !fill_ok(*fill) {
                    return Err(invalid());
                }
                let (regions, must_remove) = self
                    .layout(*page_index, false)?
                    .redaction_target(*start, *end)?;
                // A redaction that selects nothing would look applied and remove nothing.
                if must_remove.is_empty() || !regions.iter().all(rect_ok) {
                    return Err(invalid());
                }
                Ok(StoredRedaction {
                    id: id.clone(),
                    page_index: *page_index,
                    regions,
                    fill: *fill,
                    must_remove,
                })
            }
        }
    }

    /// Spec §4. `STALE_REVISION` when `base_revision` is not the current revision (so a
    /// resubmitted, already committed ChangeSet never commits twice). Otherwise every item is
    /// validated and, if none is rejected, both lists and the working copy are replaced and the
    /// revision goes up by one. Edits are handed to the applier only when nothing else was
    /// rejected, so the applier's rejections can be missing from a result that already has others.
    pub(crate) fn apply(
        &mut self,
        cs: ChangeSet,
        limits: &Limits,
        applier: &dyn EditApplier,
    ) -> Result<ChangeResult, PdfError> {
        if cs.base_revision != self.revision {
            return Err(PdfError::new(ErrorCode::StaleRevision));
        }
        let mut rejected = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        if cs.text_edits.is_some() {
            seen.extend(self.committed.redactions.iter().map(|r| r.id.clone()));
        }
        if cs.redactions.is_some() {
            seen.extend(self.committed.edits.iter().map(|e| e.id.clone()));
        }
        // An id names one operation across both lists, and may not be empty.
        let mut fresh = |id: &str| !id.is_empty() && seen.insert(id.to_owned());

        let replaced = cs.text_edits.is_some();
        let edits = cs.text_edits;
        for e in edits.iter().flatten() {
            if !fresh(&e.id) {
                rejected.push(reject(&e.id, invalid()));
            } else if let Err(err) = self.check_edit(e) {
                rejected.push(reject(&e.id, err));
            }
        }
        let mut redactions = None;
        if let Some(list) = &cs.redactions {
            let mut stored = Vec::new();
            for r in list {
                let (Redaction::Region { id, .. } | Redaction::Text { id, .. }) = r;
                if !fresh(id) {
                    rejected.push(reject(id, invalid()));
                    continue;
                }
                match self.resolve(r) {
                    Ok(s) => stored.push(s),
                    Err(err) => rejected.push(reject(id, err)),
                }
            }
            redactions = Some(stored);
        }

        let mut working = None;
        let mut layouts = Vec::new();
        if let Some(list) = edits.as_ref().filter(|l| !l.is_empty())
            && rejected.is_empty()
        {
            match applier.apply(&self.source_bytes, list, limits) {
                Ok(a) => {
                    let doc = Document::open_with_limits(a.bytes, None, false, limits.clone())?
                        .with_cancel(self.source.cancel_handle());
                    if doc.page_count() != self.source.page_count() {
                        return Err(PdfError::new(ErrorCode::Internal));
                    }
                    working = Some(doc);
                    layouts = a.layouts;
                }
                Err(rs) => rejected.extend(rs),
            }
        }
        if !rejected.is_empty() {
            return Ok(ChangeResult {
                accepted: false,
                revision: self.revision,
                rejected,
                changed_pages: Vec::new(),
                layouts: Vec::new(),
            });
        }

        let revision = self
            .revision
            .checked_add(1)
            .ok_or(PdfError::new(ErrorCode::Internal))?;
        let next = Committed {
            edits: edits.unwrap_or_else(|| self.committed.edits.clone()),
            redactions: redactions.unwrap_or_else(|| self.committed.redactions.clone()),
        };
        let mut changed = differing_pages(&self.committed.edits, &next.edits, |e| e.page_index);
        changed.extend(differing_pages(
            &self.committed.redactions,
            &next.redactions,
            |r| r.page_index,
        ));
        if next.edits.is_empty() {
            self.working = None;
        } else if working.is_some() {
            self.working = working;
        }
        self.committed = next;
        self.revision = revision;
        // A new working copy renumbers objects, so every cached glyph id would be wrong.
        self.index.advance(revision, replaced);
        Ok(ChangeResult {
            accepted: true,
            revision,
            rejected: Vec::new(),
            changed_pages: changed.into_iter().collect(),
            layouts,
        })
    }
}

/// Pages whose items differ, comparing each page's items in list order.
fn differing_pages<T: PartialEq>(old: &[T], new: &[T], page: impl Fn(&T) -> u32) -> BTreeSet<u32> {
    fn on<'a, T>(list: &'a [T], p: u32, page: &impl Fn(&T) -> u32) -> Vec<&'a T> {
        list.iter().filter(|t| page(t) == p).collect()
    }
    let pages: BTreeSet<u32> = old.iter().chain(new).map(&page).collect();
    pages
        .into_iter()
        .filter(|&p| on(old, p, &page) != on(new, p, &page))
        .collect()
}
