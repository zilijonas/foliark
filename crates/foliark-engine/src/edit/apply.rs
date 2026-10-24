//! Applying paragraph edits (spec §4, §6.3).
//!
//! The edit list replays from the source in array order. Each edit is evaluated on the working
//! copy the previous ones made (their file is reopened by the strict reader), so its rects name
//! what the user saw after them. One edit is: `prepare` the selection and its metrics, lay the
//! new text out with the font ladder, remove the selected source clusters (text only, through
//! the redaction writer path: images, vectors and backgrounds are never touched and nothing is
//! filled), then draw the planned runs on top of the page in the paragraph's color.
//!
//! Working copies are written in preserve mode (spec §8.3): annotations, forms, layers and the
//! rest of the file stay as they were, so render and export read what the user edited. Export
//! flattens or removes them on its own pass; a working copy is never returned.
//!
//! Source-font runs reuse the source font object by character code. A registered font is one
//! Identity-H subset object per (font id, document): the next edit finds it again by its
//! resource name and its recorded glyph list, embeds the union (old glyphs keep their CIDs, so
//! earlier content still reads right) and the new object replaces the old one wherever the file
//! referred to it.
//!
//! Every edit is evaluated and every failure is reported; one failure rejects the whole list and
//! nothing is kept.
use super::preview::{Planned, plan_edit};
use crate::document::{Applied, EditApplier, EditContext};
use crate::reader::object_id;
use crate::redaction::plan::RedactionPlan;
use crate::redaction::text::redact_text;
use crate::writer::fonts::{
    EmbeddedFont, UsedGlyphs, embed_registered_font, registered_resource, registered_state,
};
use crate::writer::text::{PlanFonts, SourceFonts, emit_plan};
use crate::writer::{Val, Writer};
use crate::{Binding, Committed, Document, StoredRedaction};
use foliark_model::{
    EditLayout, ErrorCode, ObjectId, PdfError, Rect, RejectedChange, RunSource, TextEdit,
};
use foliark_text::FontFace;
use hayro::hayro_syntax::object::{Dict, MaybeRef, Object};
use std::collections::BTreeSet;

/// Removal selects exactly the edit's clusters (a must-remove set), so its one region lies far
/// off every page: it can never take a neighbour out by overlap.
const OFF_PAGE: Rect = Rect {
    x: -1.0e6,
    y: -1.0e6,
    width: 1.0,
    height: 1.0,
};

pub struct ParagraphApplier;

impl EditApplier for ParagraphApplier {
    fn apply(
        &self,
        source: &Document,
        edits: &[TextEdit],
        ctx: &EditContext<'_>,
    ) -> Result<Applied, Vec<RejectedChange>> {
        let reject = |e: &TextEdit, err: PdfError| {
            let mut details = err.details.map(|d| *d).unwrap_or_default();
            details.edit_id = Some(e.id.clone());
            RejectedChange {
                id: e.id.clone(),
                code: err.code,
                details: Some(details),
            }
        };
        let mut layouts = Vec::new();
        let mut rejected = Vec::new();
        let mut state: Option<Document> = None;
        let mut bytes = Vec::new();
        for (n, e) in edits.iter().enumerate() {
            let doc = state.as_ref().unwrap_or(source);
            // A failed edit is skipped so the ones after it are still judged; the result is
            // then a rejection of the whole list anyway.
            let (out, layout) = match apply_one(doc, n, e, ctx) {
                Ok(done) => done,
                Err(err) => {
                    rejected.push(reject(e, err));
                    continue;
                }
            };
            layouts.push(layout);
            if n + 1 < edits.len() {
                match Document::open_with_limits(out.clone(), None, false, ctx.limits.clone()) {
                    Ok(next) => state = Some(next.with_cancel(source.cancel_handle())),
                    Err(_) => {
                        rejected.push(reject(e, PdfError::new(ErrorCode::Internal)));
                        continue;
                    }
                }
            }
            bytes = out;
        }
        if rejected.is_empty() {
            Ok(Applied { bytes, layouts })
        } else {
            Err(rejected)
        }
    }
}

/// A registered font already in the file: its font object and the glyphs it holds.
type Earlier = (ObjectId, Vec<(u16, String)>);

/// Registered font `res` already in the file: its font object and the glyphs it holds, and
/// whether `page` already names it in its font resources.
fn find_registered(doc: &Document, res: &str, page: usize) -> (Option<Earlier>, bool) {
    let reader = doc.reader();
    let mut found = None;
    let mut on_page = false;
    for i in 0..reader.page_count() {
        let Ok(p) = reader.page(i) else { continue };
        // /Resources is inheritable.
        let mut node = p.raw().clone();
        let fonts = loop {
            if let Some(f) = node
                .get::<Dict<'_>>("Resources")
                .and_then(|r| r.get::<Dict<'_>>("Font"))
            {
                break Some(f);
            }
            match node.get::<Dict<'_>>("Parent") {
                Some(parent) => node = parent,
                None => break None,
            }
        };
        let Some(MaybeRef::Ref(r)) = fonts.and_then(|f| f.get_raw::<Object<'_>>(res)) else {
            continue;
        };
        on_page |= i == page;
        let id = object_id(r);
        if found.is_none()
            && let Some(state) = reader
                .pdf()
                .xref()
                .get::<Dict<'_>>(r.into())
                .and_then(|d| registered_state(&d))
        {
            found = Some((id, state));
        }
    }
    (found, on_page)
}

fn apply_one(
    doc: &Document,
    n: usize,
    e: &TextEdit,
    ctx: &EditContext<'_>,
) -> Result<(Vec<u8>, EditLayout), PdfError> {
    let page = e.page_index as usize;
    let Planned {
        prepared: p,
        text,
        laid,
        ..
    } = plan_edit(doc, page, &e.rects, e.font.as_deref(), &e.text, ctx)?;
    let info = &p.info;
    let plan = laid.fit()?;

    // Remove the selected clusters. The glyphs the new text reuses from the source font must
    // count as survivors, or removal would empty their outlines.
    let committed = Committed {
        edits: Vec::new(),
        redactions: vec![StoredRedaction {
            id: e.id.clone(),
            page_index: e.page_index,
            regions: vec![OFF_PAGE],
            fill: None,
            must_remove: p.ids.clone(),
        }],
    };
    let binding = Binding {
        doc_id: String::new(),
        generation: 0,
        revision: 0,
    };
    let mut removal = RedactionPlan::build(doc, &committed, binding)?;
    let runs = || plan.lines.iter().flat_map(|l| &l.runs);
    if let Some(kept) = p.font_obj.and_then(|f| removal.font_survivors.get_mut(&f)) {
        let reused = runs()
            .filter(|r| r.source == RunSource::Embedded)
            .flat_map(|r| r.glyphs.iter().map(|g| g.gid))
            .collect::<BTreeSet<_>>();
        for code in reused {
            if let Some(g) = p.glyphs.iter().find(|g| g.char_code == code)
                && !kept.iter().any(|k| k.char_code == code)
            {
                kept.push(g.clone());
            }
        }
    }
    let mut w = Writer::new(doc.reader(), ctx.limits.clone());
    w.preserve();
    redact_text(doc, &removal, &mut w)?;

    if !plan.lines.is_empty() {
        let internal = || PdfError::new(ErrorCode::Internal);
        let mut registered = Vec::<(String, EmbeddedFont, String)>::new();
        let ids: BTreeSet<&str> = runs()
            .filter(|r| r.source == RunSource::Registered)
            .map(|r| r.font.as_str())
            .collect();
        for id in ids {
            let face = ctx.fonts.get(id).ok_or_else(internal)?;
            let res = registered_resource(id);
            let (earlier, on_page) = find_registered(doc, &res, page);
            let mut used = match &earlier {
                Some((_, glyphs)) => UsedGlyphs::from_prior(glyphs.clone()),
                None => UsedGlyphs::new(),
            };
            used.add_plan(&plan, id, &text);
            let font = embed_registered_font(&mut w, face, &used)?;
            if let Some((old, _)) = earlier {
                w.supersede(old, font.font.clone());
            }
            if !on_page {
                w.add_page_font(page, &res, font.font.clone())?;
            }
            registered.push((id.to_owned(), font, res));
        }
        let source_res = format!("FkE{n}S");
        let mut sources = SourceFonts::new();
        if let (Some(face), Some(font)) = (&p.source, p.font_obj)
            && runs().any(|r| r.source == RunSource::Embedded)
        {
            w.add_page_font(page, &source_res, Val::Src(font))?;
            sources.insert(face.id(), (source_res.as_str(), face.code_width()));
        }
        let fonts: PlanFonts<'_> = registered
            .iter()
            .map(|(id, font, res)| (id.as_str(), (res.as_str(), font)))
            .collect();
        let geometry = doc.page_geometry(page)?;
        let ops = emit_plan(
            &plan,
            &fonts,
            &sources,
            info.color,
            geometry.display_to_user.0,
        )?;
        w.append_page_content(page, &ops)?;
    }
    Ok((w.write()?, plan.edit_layout(&e.id)))
}
