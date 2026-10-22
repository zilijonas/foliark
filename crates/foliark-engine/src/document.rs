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
    ChangeResult, ChangeSet, DocumentInfo, EditLayout, ErrorCode, ErrorDetails, FontMatchRule,
    Limits, PdfError, Rect, Redaction, RejectedChange, TextEdit,
};
use foliark_text::Registry;
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
    /// `None` = no fill: the content is removed and nothing is painted over it.
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

/// What an applier reads from the engine besides the document.
pub struct EditContext<'a> {
    pub limits: &'a Limits,
    pub fonts: &'a Registry,
    pub font_match: &'a [FontMatchRule],
}

/// The seam [`crate::edit::ParagraphApplier`] fills: replay `edits` from `source` in array order
/// (each edit's rects are evaluated on the state after the previous ones) and return the new
/// working copy, or the rejection of the edit that cannot apply. Called only with a non-empty
/// list whose edits passed structural validation. It must not keep state: a rejection or a
/// later failure discards whatever it produced.
pub trait EditApplier {
    fn apply(
        &self,
        source: &Document,
        edits: &[TextEdit],
        ctx: &EditContext<'_>,
    ) -> Result<Applied, Vec<RejectedChange>>;
}

/// An engine without text edits: every edit is rejected `INVALID_ARGUMENT` with
/// `details.feature = "textEdits"` (the convention of an op this build does not do).
pub struct NotBuilt;

impl EditApplier for NotBuilt {
    fn apply(
        &self,
        _: &Document,
        edits: &[TextEdit],
        _: &EditContext<'_>,
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
    /// Committed edits whose layout used a font other than the source's (`edits.approximate`).
    pub(crate) approximate: u32,
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
            approximate: 0,
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
                if !rect_ok(rect) || !fill_ok(fill.flatten()) {
                    return Err(invalid());
                }
                Ok(StoredRedaction {
                    id: id.clone(),
                    page_index: *page_index,
                    regions: vec![*rect],
                    fill: fill.unwrap_or(Some([0.0; 3])),
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
                if !fill_ok(fill.flatten()) {
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
                    fill: fill.unwrap_or(Some([0.0; 3])),
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
        ctx: &EditContext<'_>,
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
            match applier.apply(&self.source, list, ctx) {
                Ok(a) => {
                    let doc = Document::open_with_limits(a.bytes, None, false, ctx.limits.clone())?
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
        // A new working copy renumbers objects, so every stored must-remove id must be found
        // again in it (spec §4); one that cannot be is refused, never dropped.
        if replaced && rejected.is_empty() && (self.working.is_some() || working.is_some()) {
            let mut list = redactions
                .take()
                .unwrap_or_else(|| self.committed.redactions.clone());
            let old = self.working.as_ref().unwrap_or(&self.source);
            let new = working.as_ref().unwrap_or(&self.source);
            rejected.extend(reresolve(&mut self.index, old, new, &mut list));
            redactions = Some(list);
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
        // Re-resolved ids differ after any edit; what the user changed is the intent.
        let intent = |l: &[StoredRedaction]| -> Vec<_> {
            l.iter()
                .map(|r| (r.page_index, r.id.clone(), r.regions.clone(), r.fill))
                .collect()
        };
        let mut changed = differing_pages(&self.committed.edits, &next.edits, |e| e.page_index);
        changed.extend(differing_pages(
            &intent(&self.committed.redactions),
            &intent(&next.redactions),
            |r| r.0,
        ));
        if next.edits.is_empty() {
            self.working = None;
        } else if working.is_some() {
            self.working = working;
        }
        if replaced {
            self.approximate = layouts.iter().filter(|l| l.approximate).count() as u32;
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

/// Each cluster of a page as (text, display bounds), in cluster order.
fn glyphs(l: &TextLayout) -> Vec<(String, [f64; 4])> {
    let units: Vec<u16> = l.page.text.encode_utf16().collect();
    l.page
        .clusters
        .iter()
        .flatten()
        .map(|c| {
            let text = String::from_utf16_lossy(&units[c.start as usize..c.end as usize]);
            let b = c.quad.bounds();
            (text, [b.x, b.y, b.width, b.height])
        })
        .collect()
}

/// The must-remove clusters of `r`, named in `old` (the working copy they were resolved on),
/// found again in `new` by text and geometry (0.05 pt), one cluster each.
fn remap(
    index: &mut TextIndex,
    old: &Document,
    new: &Document,
    r: &StoredRedaction,
) -> Result<Vec<ClusterId>, PdfError> {
    let internal = || PdfError::new(ErrorCode::Internal);
    let before = index.layout(old, r.page_index, false)?;
    let all = glyphs(before);
    let wanted: Vec<&(String, [f64; 4])> = r
        .must_remove
        .iter()
        .map(|id| {
            before
                .meta
                .iter()
                .position(|m| &m.id == id)
                .map(|i| &all[i])
        })
        .collect::<Option<_>>()
        .ok_or_else(internal)?;
    let after = new.text_layout(r.page_index as usize, false)?;
    let have = glyphs(&after);
    let mut taken = vec![false; have.len()];
    wanted
        .iter()
        .map(|(text, b)| {
            let j = (0..have.len()).find(|&j| {
                !taken[j]
                    && have[j].0 == *text
                    && have[j].1.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.05)
            })?;
            taken[j] = true;
            Some(after.meta[j].id.clone())
        })
        .collect::<Option<_>>()
        .ok_or_else(|| {
            PdfError::with(
                ErrorCode::InvalidArgument,
                ErrorDetails {
                    feature: Some("redactionTargetEdited".into()),
                    pages: Some(vec![r.page_index]),
                    ..Default::default()
                },
            )
        })
}

/// Re-resolve every text redaction of `list` against `new`; one rejection per redaction whose
/// clusters are not all there (an edit removed or moved the text it targets).
fn reresolve(
    index: &mut TextIndex,
    old: &Document,
    new: &Document,
    list: &mut [StoredRedaction],
) -> Vec<RejectedChange> {
    let mut rejected = Vec::new();
    for r in list.iter_mut().filter(|r| !r.must_remove.is_empty()) {
        match remap(index, old, new, r) {
            Ok(ids) => r.must_remove = ids,
            Err(e) => rejected.push(reject(&r.id, e)),
        }
    }
    rejected
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
