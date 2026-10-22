//! Applying paragraph edits (spec §4, §6.3).
//!
//! The edit list replays from the source in array order. Each edit is evaluated on the working
//! copy the previous ones made (their file is reopened by the strict reader), so its rects name
//! what the user saw after them. One edit is: `prepare` the selection and its metrics, lay the
//! new text out with the font ladder, remove the selected source clusters (text only, through
//! the redaction writer path: images, vectors and backgrounds are never touched and nothing is
//! filled), then draw the planned runs on top of the page in the paragraph's color.
//!
//! Source-font runs reuse the source font object by character code. Registered fonts are
//! embedded as Identity-H subsets, one font object per (font id, edit file): each edit starts
//! from a fresh writer, so a font used by several edits is embedded once per edit.
//!
//! Any edit that cannot apply rejects the whole list; nothing is kept.
use crate::document::{Applied, EditApplier, EditContext};
use crate::export::preflight::annotations;
use crate::redaction::plan::RedactionPlan;
use crate::redaction::text::redact_text;
use crate::writer::fonts::{EmbeddedFont, UsedGlyphs, embed_registered_font};
use crate::writer::text::{PlanFonts, SourceFonts, emit_plan};
use crate::writer::{Val, Writer};
use crate::{Binding, Committed, Document, StoredRedaction};
use foliark_model::{
    EditLayout, ErrorCode, ErrorDetails, PdfError, Rect, RejectedChange, RunSource, TextEdit,
};
use foliark_text::{FontFace, Ladder, LayoutParams, layout_paragraph, normalize};
use hayro::hayro_syntax::object::Dict;
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
            vec![RejectedChange {
                id: e.id.clone(),
                code: err.code,
                details: Some(details),
            }]
        };
        if let Some(pages) = preserve_gap(source) {
            let err = PdfError::with(
                ErrorCode::InvalidArgument,
                ErrorDetails {
                    feature: Some("editPreserveMode".into()),
                    pages: (!pages.is_empty()).then_some(pages),
                    ..Default::default()
                },
            );
            return Err(edits
                .iter()
                .map(|e| reject(e, err.clone()).remove(0))
                .collect());
        }
        let mut layouts = Vec::new();
        let mut state: Option<Document> = None;
        let mut bytes = Vec::new();
        for (n, e) in edits.iter().enumerate() {
            let doc = state.as_ref().unwrap_or(source);
            let (out, layout) = apply_one(doc, n, e, ctx).map_err(|err| reject(e, err))?;
            layouts.push(layout);
            if n + 1 < edits.len() {
                let next = Document::open_with_limits(out.clone(), None, false, ctx.limits.clone())
                    .map_err(|_| reject(e, PdfError::new(ErrorCode::Internal)))?;
                state = Some(next.with_cancel(source.cancel_handle()));
            }
            bytes = out;
        }
        Ok(Applied { bytes, layouts })
    }
}

/// The working copy is written by the allowlist writer (preserve mode, spec §8.3, is not built),
/// which drops annotations, form fields and optional-content settings. Render and export read
/// the working copy, so an edit on a file that has any of them is refused, never done lossily.
/// `Some(pages)` names the pages with visible annotations or widgets.
fn preserve_gap(doc: &Document) -> Option<Vec<u32>> {
    let reader = doc.reader();
    let annots = annotations(reader);
    let x = reader.pdf().xref();
    let cat = x.get::<Dict<'_>>(x.root_id());
    let in_catalog =
        cat.is_some_and(|c| c.contains_key("OCProperties") || c.contains_key("AcroForm"));
    if annots.visible == 0 && annots.widget_pages.is_empty() && !in_catalog {
        return None;
    }
    let mut pages: Vec<u32> = annots
        .visible_pages
        .into_iter()
        .chain(annots.widget_pages)
        .collect();
    pages.sort_unstable();
    pages.dedup();
    Some(pages)
}

fn apply_one(
    doc: &Document,
    n: usize,
    e: &TextEdit,
    ctx: &EditContext<'_>,
) -> Result<(Vec<u8>, EditLayout), PdfError> {
    let page = e.page_index as usize;
    let p = doc.prepare(
        page,
        &e.rects,
        e.font.as_deref(),
        false,
        ctx.fonts,
        ctx.font_match,
    )?;
    let info = &p.info;
    let text = normalize(&e.text)?;
    let ladder = Ladder {
        registry: ctx.fonts,
        caller: e.font.as_deref(),
        source: p.source.as_ref().map(|s| s as &dyn FontFace),
        source_name: &p.source_name,
        flags: p.flags,
        font_match: ctx.font_match,
    };
    let plan = layout_paragraph(
        &text,
        &LayoutParams {
            font_size: info.font_size,
            wrap_width: info.wrap_width,
            max_lines: info.max_lines,
            line_step: info.line_step,
            first_baseline: info.first_baseline,
            origin_x: info.origin_x,
        },
        &ladder,
    )?;

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
    redact_text(doc, &removal, &mut w)?;

    if !plan.lines.is_empty() {
        let internal = || PdfError::new(ErrorCode::Internal);
        let mut registered = Vec::<(String, EmbeddedFont, String)>::new();
        let ids: BTreeSet<&str> = runs()
            .filter(|r| r.source == RunSource::Registered)
            .map(|r| r.font.as_str())
            .collect();
        for (k, id) in ids.into_iter().enumerate() {
            let face = ctx.fonts.get(id).ok_or_else(internal)?;
            let mut used = UsedGlyphs::new();
            used.add_plan(&plan, id, &text);
            let font = embed_registered_font(&mut w, face, &used)?;
            let res = format!("FkE{n}R{k}");
            w.add_page_font(page, &res, font.font.clone())?;
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
