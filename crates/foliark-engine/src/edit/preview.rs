//! Edit previews (ADR 0004): the layout an edit would commit, computed by the same function
//! `apply_one` calls, so a preview and the committed text cannot disagree.
use crate::Document;
use crate::document::EditContext;
use crate::edit::Prepared;
use foliark_model::{EditPreview, PdfError, PreviewGlyph, PreviewLine, Rect};
use foliark_text::{FontFace, Ladder, Laid, LayoutParams, layout_unbounded, normalize};

/// Everything an edit needs before anything is written: the selection, the normalized text and
/// its layout with the budget not yet enforced.
pub(super) struct Planned {
    pub prepared: Prepared,
    pub text: String,
    pub laid: Laid,
}

pub(super) fn plan_edit(
    doc: &Document,
    page: usize,
    rects: &[Rect],
    font: Option<&str>,
    text: &str,
    ctx: &EditContext<'_>,
) -> Result<Planned, PdfError> {
    let prepared = doc.prepare(page, rects, font, false, ctx.fonts, ctx.font_match)?;
    let text = normalize(text)?;
    let info = &prepared.info;
    let ladder = Ladder {
        registry: ctx.fonts,
        caller: font,
        source: prepared.source.as_ref().map(|s| s as &dyn FontFace),
        source_name: &prepared.source_name,
        flags: prepared.flags,
        font_match: ctx.font_match,
    };
    let laid = layout_unbounded(
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
    Ok(Planned {
        prepared,
        text,
        laid,
    })
}

/// `doc_id` and `revision` are left for the session layer, as in `prepare_paragraph`.
pub fn layout_edit(
    doc: &Document,
    page: usize,
    rects: &[Rect],
    font: Option<&str>,
    text: &str,
    glyphs: bool,
    ctx: &EditContext<'_>,
) -> Result<EditPreview, PdfError> {
    let Planned {
        prepared,
        text,
        laid,
    } = plan_edit(doc, page, rects, font, text, ctx)?;
    let text16: Vec<u16> = text.encode_utf16().collect();
    let (mut lines, mut boxes) = (Vec::new(), Vec::new());
    for (n, line) in laid.plan.lines.iter().enumerate() {
        let (mut x, mut span) = (line.x, (u32::MAX, 0));
        for run in &line.runs {
            for g in &run.glyphs {
                if glyphs {
                    boxes.push(PreviewGlyph {
                        line: n as u32,
                        font: run.font.clone(),
                        gid: g.gid,
                        start: g.cluster.start,
                        end: g.cluster.end,
                        x,
                        width: g.x_advance,
                    });
                }
                span = (span.0.min(g.cluster.start), span.1.max(g.cluster.end));
                x += g.x_advance;
            }
        }
        lines.push(PreviewLine {
            text: String::from_utf16_lossy(
                text16.get(span.0 as usize..span.1 as usize).unwrap_or(&[]),
            ),
            baseline: line.baseline_y,
            x: line.x,
            width: x - line.x,
        });
    }
    let layout = laid.plan.edit_layout("");
    Ok(EditPreview {
        doc_id: String::new(),
        revision: 0,
        font_registry_version: ctx.fonts.version(),
        font_size: prepared.info.font_size,
        lines,
        needed_lines: laid.needed_lines(),
        max_lines: laid.max_lines,
        fits: laid.fits(),
        approximate: layout.approximate,
        runs: layout.runs,
        fingerprint: layout.fingerprint,
        glyphs: glyphs.then_some(boxes),
    })
}
