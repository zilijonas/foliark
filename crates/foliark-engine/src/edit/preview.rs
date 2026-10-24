//! Edit previews (ADR 0004): the layout an edit would commit, computed by the same function
//! `apply_one` calls, so a preview and the committed text cannot disagree.
use super::paragraph::{style_spans, text_runs};
use super::runs::carry;
use crate::Document;
use crate::document::EditContext;
use crate::edit::Prepared;
use foliark_model::{
    EditPreview, ErrorCode, ErrorDetails, FontStyle, PdfError, PreviewGlyph, PreviewLine, Rect,
    StyleSpan,
};
use foliark_text::{Laid, LayoutParams, layout_runs, normalize};

/// Everything an edit needs before anything is written: the selection, the normalized text and
/// its layout with the budget not yet enforced. With style segments (ADR 0012) these are the
/// changed part's: `text` is the part of the edit's text that replaces it, starting `at16`
/// UTF-16 units into the whole.
pub(super) struct Planned {
    pub prepared: Prepared,
    pub text: String,
    pub at16: u32,
    pub laid: Laid,
    /// The look of every character of the whole text (ADR 0013), as UTF-16 spans.
    pub styles: Vec<StyleSpan>,
}

/// ADR 0012: the part of a selection an edit changes. `text` is split at LF and lined up with
/// `originalText`'s lines: a segment boundary keeps its place when the line count is unchanged,
/// or the line before it is in the common prefix, or the line after it in the common suffix;
/// otherwise the segments on both sides go together. The changed part runs from the first
/// segment whose lines differ to the last: its clusters, its new text and that text's UTF-16
/// offset. `None` lays out the whole selection (one segment, unmapped text, or all changed).
fn changed_part(p: &Prepared, text: &str) -> Option<(Vec<usize>, String, u32)> {
    let segs = &p.segments;
    if segs.len() < 2 {
        return None;
    }
    // Normalized as the edit's text is, so a mark the page draws apart reads as the user sees it.
    let original = normalize(p.info.original_text.as_deref()?).ok()?;
    let old: Vec<&str> = original.split('\n').collect();
    let new: Vec<&str> = text.split('\n').collect();
    let pre = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suf = (old.iter().rev().zip(new.iter().rev()))
        .take_while(|(a, b)| a == b)
        .count()
        .min(old.len().min(new.len()) - pre);
    // Each kept boundary as (old line, new line) where the next segment starts; the ends too.
    let mut cuts = vec![(0, 0)];
    for s in &segs[1..] {
        let j = s.line;
        if old.len() == new.len() || j <= pre {
            cuts.push((j, j));
        } else if j >= old.len() - suf {
            cuts.push((j, j + new.len() - old.len()));
        }
    }
    cuts.push((old.len(), new.len()));
    let changed: Vec<usize> = (0..cuts.len() - 1)
        .filter(|&k| old[cuts[k].0..cuts[k + 1].0] != new[cuts[k].1..cuts[k + 1].1])
        .collect();
    let (a, b) = match (changed.first(), changed.last()) {
        (Some(&a), Some(&b)) => (cuts[a], cuts[b + 1]),
        // Nothing changed: nothing is laid out or removed.
        _ => return Some((Vec::new(), String::new(), 0)),
    };
    if (a.0, b.0) == (0, old.len()) {
        return None;
    }
    let ids = segs
        .iter()
        .filter(|s| (a.0..b.0).contains(&s.line))
        .flat_map(|s| s.clusters.iter().copied())
        .collect();
    let at16 = new[..a.1]
        .iter()
        .map(|l| l.encode_utf16().count() + 1)
        .sum::<usize>();
    Some((ids, new[a.1..b.1].join("\n"), at16 as u32))
}

pub(super) fn plan_edit(
    doc: &Document,
    page: usize,
    rects: &[Rect],
    font: Option<&str>,
    text: &str,
    ctx: &EditContext<'_>,
) -> Result<Planned, PdfError> {
    let whole = doc.prepare(page, rects, font, false, ctx.fonts, ctx.font_match)?;
    let text = normalize(text)?;
    // ADR 0013: the look every character of the edited text is drawn in.
    let looks = match whole.info.original_text.as_deref() {
        Some(original) => carry(original, &whole.text_looks, &text, whole.main),
        None => vec![whole.main; text.chars().count()],
    };
    let styles = style_spans(&text, &looks);
    let (prepared, part, at16) = match changed_part(&whole, &text) {
        None => (whole, text.clone(), 0),
        Some((ids, part, at16)) if ids.is_empty() => (
            Prepared {
                ids: Vec::new(),
                ..whole
            },
            part,
            at16,
        ),
        Some((ids, part, at16)) => {
            // The part's own metrics; looks stay the whole selection's, which `looks` indexes.
            let own = doc.prepare_clusters(page, &ids, font, ctx.fonts, ctx.font_match)?;
            let index = own.looks[own.main].index;
            let main = whole.looks.iter().position(|l| l.index == index);
            (
                Prepared {
                    main: main.ok_or_else(|| PdfError::new(ErrorCode::Internal))?,
                    looks: whole.looks,
                    ..own
                },
                part,
                at16,
            )
        }
    };
    let skip = text
        .chars()
        .scan(0, |at, c| {
            *at += c.len_utf16() as u32;
            Some(*at)
        })
        .take_while(|&end| end <= at16)
        .count();
    let part_looks = &looks[skip..skip + part.chars().count()];
    let info = &prepared.info;
    // A caller font draws the paragraph's own style; a run in another weight or style takes the
    // caller family's face in that weight and style, and refuses when it is not registered.
    let main = &prepared.looks[prepared.main];
    let mut missing = String::new();
    let caller_for = |k: usize| -> Option<Option<&str>> {
        let Some(id) = font else { return Some(None) };
        let (look, want) = (prepared.looks[k].flags(), main.flags());
        if (look.bold, look.italic) == (want.bold, want.italic) {
            return Some(Some(id));
        }
        let family = &ctx.fonts.get(id)?.info().family;
        let face = ctx.fonts.lookup(family, look.bold, look.italic)?.info();
        let style = if look.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        (face.weight == if look.bold { 700 } else { 400 } && face.style == style)
            .then_some(Some(face.id.as_str()))
    };
    for (c, &k) in part.chars().zip(part_looks) {
        if caller_for(k).is_none() && !missing.contains(c) && !c.is_whitespace() {
            missing.push(c);
        }
    }
    if !missing.is_empty() {
        return Err(PdfError::with(
            ErrorCode::EditNoFont,
            ErrorDetails {
                chars: Some(missing),
                ..Default::default()
            },
        ));
    }
    let runs = text_runs(&part, part_looks, &prepared.looks, prepared.main, |k| {
        let ladder = prepared.looks[k].ladder(ctx.fonts, ctx.font_match);
        foliark_text::Ladder {
            caller: caller_for(k).flatten(),
            ..ladder
        }
    });
    let laid = layout_runs(
        &part,
        &LayoutParams {
            font_size: info.font_size,
            wrap_width: info.wrap_width,
            max_lines: info.max_lines,
            line_step: info.line_step,
            first_baseline: info.first_baseline,
            origin_x: info.origin_x,
        },
        &runs,
    )?;
    Ok(Planned {
        prepared,
        text: part,
        at16,
        laid,
        styles,
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
        at16,
        laid,
        styles,
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
                        start: at16 + g.cluster.start,
                        end: at16 + g.cluster.end,
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
        styles,
        glyphs: glyphs.then_some(boxes),
    })
}
