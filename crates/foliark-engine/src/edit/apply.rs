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
use crate::document::{Applied, EditApplier, EditContext, map_rect, shifted, steps};
use crate::interpret::ItemKind;
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
    EditLayout, ErrorCode, ErrorDetails, ObjectId, PageShift, PdfError, Rect, RejectedChange,
    RunSource, TextEdit,
};
use foliark_text::FontFace;
use hayro::hayro_syntax::object::{Dict, MaybeRef};
use std::collections::BTreeSet;
use unicode_normalization::UnicodeNormalization;

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
        let mut shifts = Vec::new();
        for (n, e) in edits.iter().enumerate() {
            let doc = state.as_ref().unwrap_or(source);
            // ADR 0015: the rects are in the frame of the committed list (after its edits before
            // this one, or after all of them for a new id); map them to the frame after the new
            // edits before this one.
            let from: Vec<PageShift> = match ctx.prior.iter().position(|p| p.0 == e.id) {
                Some(j) => ctx.prior[..j].iter().filter_map(|p| p.1).collect(),
                None => ctx.prior.iter().filter_map(|p| p.1).collect(),
            };
            let done: Vec<PageShift> = shifts.iter().flatten().copied().collect();
            let to_frame = steps(&from, &done);
            let mapped: TextEdit;
            let e = match (e.rects.iter())
                .map(|r| map_rect(e.page_index, *r, &to_frame))
                .collect::<Option<Vec<_>>>()
            {
                Some(rects) if to_frame.is_empty() || rects == e.rects => e,
                Some(rects) => {
                    mapped = TextEdit { rects, ..e.clone() };
                    &mapped
                }
                None => {
                    rejected.push(reject(e, shifted("editTargetShifted", e.page_index)));
                    continue;
                }
            };
            // A failed edit is skipped so the ones after it are still judged; the result is
            // then a rejection of the whole list anyway.
            let (out, next, layout, shift) = match apply_one(doc, n, e, ctx) {
                Ok(done) => done,
                Err(err) => {
                    rejected.push(reject(e, err));
                    continue;
                }
            };
            layouts.push(layout);
            shifts.push(shift);
            state = Some(next);
            bytes = out;
        }
        if rejected.is_empty() {
            Ok(Applied {
                bytes,
                layouts,
                shifts,
            })
        } else {
            Err(rejected)
        }
    }
}

/// A registered font already in the file: its font object and the glyphs it holds.
type Earlier = (ObjectId, Vec<(u16, String)>);

/// Registered font `font_id` already in the file with its recorded state proven
/// ([`registered_state`]): its font object and glyphs, and the name page `page` draws that same
/// object by. A same-named font without the record (an exported file's) is not it.
fn find_registered(
    doc: &Document,
    font_id: &str,
    page: usize,
) -> (Option<Earlier>, Option<String>) {
    let reader = doc.reader();
    let xref = reader.pdf().xref();
    let mut found: Option<Earlier> = None;
    let mut here: Vec<(String, ObjectId)> = Vec::new();
    for i in 0..reader.page_count() {
        let Ok(p) = reader.page(i) else { continue };
        // /Resources is inheritable.
        let mut node = p.raw().clone();
        let fonts = loop {
            if let Some(r) = node.get::<Dict<'_>>("Resources") {
                break r.get::<Dict<'_>>("Font");
            }
            match node.get::<Dict<'_>>("Parent") {
                Some(parent) => node = parent,
                None => break None,
            }
        };
        for (name, entry) in fonts.iter().flat_map(|f| f.entries()) {
            let MaybeRef::Ref(r) = entry else { continue };
            let id = object_id(r);
            if i == page {
                here.push((name.as_str().to_owned(), id));
            }
            if found.is_none()
                && let Some(state) = xref
                    .get::<Dict<'_>>(r.into())
                    .and_then(|d| registered_state(&d, font_id))
            {
                found = Some((id, state));
            }
        }
    }
    let on_page = found.as_ref().and_then(|(old, _)| {
        here.into_iter()
            .find(|(_, id)| id == old)
            .map(|(name, _)| name)
    });
    (found, on_page)
}

fn apply_one(
    doc: &Document,
    n: usize,
    e: &TextEdit,
    ctx: &EditContext<'_>,
) -> Result<(Vec<u8>, Document, EditLayout, Option<PageShift>), PdfError> {
    let page = e.page_index as usize;
    let reflow = e.reflow.unwrap_or_default();
    let Planned {
        prepared: p,
        text,
        laid,
        push,
        blocked,
        ..
    } = plan_edit(doc, page, &e.rects, e.font.as_deref(), &e.text, reflow, ctx)?;
    if let Some(Some(feature)) = blocked {
        return Err(PdfError::with(
            ErrorCode::EditNoFit,
            ErrorDetails {
                needed_lines: Some(laid.needed_lines()),
                max_lines: Some(laid.max_lines),
                feature: Some(feature.into()),
                ..Default::default()
            },
        ));
    }
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
    // Each look's font (ADR 0013): the codes its runs reuse.
    let reused = |face: &dyn FontFace| {
        runs()
            .filter(|r| r.source == RunSource::Embedded && r.font == face.id())
            .flat_map(|r| r.glyphs.iter().map(|g| g.gid))
            .collect::<BTreeSet<_>>()
    };
    for look in &p.looks {
        let (Some(face), Some(kept)) = (
            &look.source,
            look.font_obj
                .and_then(|f| removal.font_survivors.get_mut(&f)),
        ) else {
            continue;
        };
        for code in reused(face) {
            if let Some(g) = look.glyphs.iter().find(|g| g.char_code == code)
                && !kept.iter().any(|k| k.char_code == code)
            {
                kept.push(g.clone());
            }
        }
    }
    let mut w = Writer::new(doc.reader(), ctx.limits.clone());
    w.preserve();
    redact_text(doc, &removal, &mut w)?;
    // ADR 0015: what lies below moves in the same pass, before the new text is drawn on top.
    if let Some(push) = &push {
        push.write(&mut w);
    }

    if !plan.lines.is_empty() {
        let internal = || PdfError::new(ErrorCode::Internal);
        let mut registered = Vec::<(String, EmbeddedFont, String)>::new();
        let ids: BTreeSet<&str> = runs()
            .filter(|r| r.source == RunSource::Registered)
            .map(|r| r.font.as_str())
            .collect();
        for id in ids {
            let face = ctx.fonts.get(id).ok_or_else(internal)?;
            let (earlier, on_page) = find_registered(doc, id, page);
            let mut used = match &earlier {
                Some((_, glyphs)) => UsedGlyphs::from_prior(glyphs.clone()),
                None => UsedGlyphs::new(),
            };
            used.add_plan(&plan, id, &text);
            let font = embed_registered_font(&mut w, face, &used)?;
            if let Some((old, _)) = earlier {
                w.supersede(old, font.font.clone());
            }
            let res = match on_page {
                Some(name) => name,
                None => w.add_page_font(page, &registered_resource(id), font.font.clone())?,
            };
            registered.push((id.to_owned(), font, res));
        }
        let mut source_res: Vec<(&str, String, usize)> = Vec::new();
        for (k, look) in p.looks.iter().enumerate() {
            if let (Some(face), Some(font)) = (&look.source, look.font_obj)
                && !source_res.iter().any(|(id, ..)| *id == face.id())
                && !reused(face).is_empty()
            {
                let res = w.add_page_font(page, &format!("FkE{n}S{k}"), Val::Src(font))?;
                source_res.push((face.id(), res, face.code_width()));
            }
        }
        let sources: SourceFonts<'_> = (source_res.iter())
            .map(|(id, res, width)| (*id, (res.as_str(), *width)))
            .collect();
        let fonts: PlanFonts<'_> = registered
            .iter()
            .map(|(id, font, res)| (id.as_str(), (res.as_str(), font)))
            .collect();
        let geometry = doc.page_geometry(page)?;
        let ops = emit_plan(&plan, &fonts, &sources, geometry.display_to_user.0)?;
        w.append_page_content(page, &ops)?;
    }
    let out = w.write()?;
    let written = Document::open_with_limits(out.clone(), None, false, ctx.limits.clone())
        .map_err(|_| PdfError::new(ErrorCode::Internal))?
        .with_cancel(doc.cancel_handle());
    if !plan.lines.is_empty() {
        read_back(&written, page, &text)?;
    }
    Ok((out, written, plan.edit_layout(&e.id), push.map(|p| p.shift)))
}

/// The self-check of a written edit: the glyphs the page's last content stream draws (the new
/// text, `append_page_content`'s tail) must read as `text`, whitespace aside. Anything else (a
/// glyph drawn through the wrong font object, a lost glyph) is an engine defect: the edit is
/// refused with `INTERNAL` (feature `editReadBack`) and nothing is kept.
fn read_back(doc: &Document, page: usize, text: &str) -> Result<(), PdfError> {
    let last = doc.reader().page_contents(page)?.last().map(|c| c.obj);
    let model = doc.page_model(page)?;
    let drawn: String = (model.items.iter())
        .filter(|i| Some(i.loc.stream) == last && i.path.0.is_empty())
        .filter_map(|i| match &i.kind {
            ItemKind::Glyph(g) => Some(g.unicode.as_deref().unwrap_or("\u{FFFD}")),
            _ => None,
        })
        .collect();
    // Compatibility forms and the hyphen family fold: a font may draw U+00AD or U+2010 with its
    // hyphen glyph, which reads back as '-'.
    let squash = |s: &str| -> String {
        (s.nfkc().filter(|c| !c.is_whitespace()))
            .map(|c| match c {
                '\u{ad}' | '\u{2010}' | '\u{2011}' => '-',
                c => c,
            })
            .collect()
    };
    if squash(&drawn) == squash(text) {
        Ok(())
    } else {
        Err(PdfError::with(
            ErrorCode::Internal,
            ErrorDetails {
                feature: Some("editReadBack".into()),
                ..Default::default()
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One page whose only content stream shows `shown` in Helvetica.
    fn page(shown: &str) -> Document {
        let content = format!("BT /F1 10 Tf 20 300 Td ({shown}) Tj ET");
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        ];
        let mut pdf = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
        for o in offsets {
            pdf.extend(format!("{o:010} 00000 n \n").bytes());
        }
        pdf.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .bytes(),
        );
        Document::open_with_limits(pdf, None, false, Default::default()).unwrap()
    }

    #[test]
    fn read_back_refuses_text_that_differs_from_the_plan() {
        let doc = page("Paid on 2026-11-12");
        assert!(read_back(&doc, 0, "Paid on\n2026-11-12").is_ok());
        let err = read_back(&doc, 0, "Paid on 2026-11-13").unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(
            err.details.and_then(|d| d.feature).as_deref(),
            Some("editReadBack")
        );
    }
}
