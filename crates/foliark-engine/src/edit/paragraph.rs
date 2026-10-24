//! `prepareParagraph` (spec §6.1, §6.2, §6.4): which clusters a paragraph edit selects, the
//! fixed budget around them (wrap width, line count, height) and the font it will use.
//! Everything is in display space and computed from the page as it is, never from the edit.
use crate::Document;
use crate::interpret::{GlyphItem, ItemKind, SemanticPage};
use crate::text::{ClusterId, SourceFontFace, TextLayout};
use foliark_model::{
    Cluster, EditorFace, ErrorCode, ErrorDetails, FontMatchRule, FontResolution, FontRule,
    GROUPING, ObjectId, ParagraphInfo, ParagraphSegment, PdfError, Quad, Rect, StyleRun,
};
use foliark_text::{
    Candidate, FontFace, Ladder, LayoutParams, Registry, StyleFlags, layout_unbounded,
};

const INVISIBLE: u32 = 1;
const UNMAPPED: u32 = 2;

fn bounds(q: &Quad) -> [f64; 4] {
    let xs = q.0.map(|p| p.x);
    let ys = q.0.map(|p| p.y);
    let fold = |v: [f64; 4], f: fn(f64, f64) -> f64| v.into_iter().reduce(f).expect("4 points");
    [
        fold(xs, f64::min),
        fold(ys, f64::min),
        fold(xs, f64::max),
        fold(ys, f64::max),
    ]
}

/// §6.1: the visible clusters whose quad centre lies in any rect, in text order. Invisible
/// (render mode 3/7, OCR) clusters are never selected: they have no style to edit and removing
/// them would not change what the page shows.
pub fn select_clusters(layout: &TextLayout, rects: &[Rect]) -> Vec<usize> {
    let clusters = layout.page.clusters.as_deref().unwrap_or_default();
    (0..clusters.len())
        .filter(|&i| {
            let c = &clusters[i];
            let b = bounds(&c.quad);
            let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
            c.flags & INVISIBLE == 0
                && rects.iter().any(|r| {
                    (r.x..=r.x + r.width).contains(&cx) && (r.y..=r.y + r.height).contains(&cy)
                })
        })
        .collect()
}

/// §6.1: baseline direction (1,0) and no skew, within 0.001. A zero-width glyph has no
/// direction of its own and is judged by its height edge alone.
fn upright(q: &Quad) -> bool {
    let unit = |a: usize, b: usize| {
        let (dx, dy) = (q.0[b].x - q.0[a].x, q.0[b].y - q.0[a].y);
        let len = dx.hypot(dy);
        (len > 1e-9).then(|| (dx / len, dy / len))
    };
    unit(0, 1).is_none_or(|(x, y)| x > 0.0 && y.abs() <= 0.001)
        && unit(0, 3).is_none_or(|(x, y)| y > 0.0 && x.abs() <= 0.001)
}

fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// One visual line of the selection.
struct Line {
    baseline: f64,
    /// Clusters left to right.
    clusters: Vec<usize>,
}

/// Selected clusters to lines by baseline (within half the font size, so superscripts stay on
/// their line and an overprinted copy joins the original), top to bottom.
fn lines(sel: &[usize], clusters: &[Cluster], baseline: &[f64], styles: &[StyleRun]) -> Vec<Line> {
    let mut order = sel.to_vec();
    order.sort_by(|&a, &b| baseline[a].total_cmp(&baseline[b]));
    let mut out: Vec<Line> = Vec::new();
    for i in order {
        let tol = styles[clusters[i].style_index as usize].size * GROUPING.same_line_perp;
        match out.last_mut() {
            Some(l) if baseline[i] - baseline[l.clusters[0]] <= tol => l.clusters.push(i),
            _ => out.push(Line {
                baseline: 0.0,
                clusters: vec![i],
            }),
        }
    }
    for l in &mut out {
        l.clusters.sort_by(|&a, &b| {
            bounds(&clusters[a].quad)[0].total_cmp(&bounds(&clusters[b].quad)[0])
        });
        l.baseline = baseline[l.clusters[0]];
    }
    out
}

fn overlap_fraction(a: &Quad, b: &Quad) -> f64 {
    let (a, b) = (bounds(a), bounds(b));
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let smaller = ((a[2] - a[0]) * (a[3] - a[1])).min((b[2] - b[0]) * (b[3] - b[1]));
    if smaller > 0.0 { w * h / smaller } else { 0.0 }
}

struct Part {
    text: String,
    x0: f64,
    x1: f64,
    h: f64,
}

/// A line's text: overprinted duplicates dropped (same Unicode over >= 50% of the previous
/// kept cluster, which repairs faux bold), a space wherever the pen gap is a word gap.
fn line_part(l: &Line, clusters: &[Cluster], text: &[Vec<u16>], styles: &[StyleRun]) -> Part {
    let mut out = Part {
        text: String::new(),
        x0: f64::INFINITY,
        x1: f64::NEG_INFINITY,
        h: 0.0,
    };
    let mut prev: Option<usize> = None;
    for &i in &l.clusters {
        let c = &clusters[i];
        let b = bounds(&c.quad);
        if let Some(p) = prev {
            let pc = &clusters[p];
            if text[p] == text[i] && overlap_fraction(&pc.quad, &c.quad) >= 0.5 {
                continue;
            }
            let em = styles[c.style_index as usize].size;
            if b[0] - out.x1 >= GROUPING.word_gap * em {
                out.text.push(' ');
            }
        }
        out.text.push_str(&String::from_utf16_lossy(&text[i]));
        out.x0 = out.x0.min(b[0]);
        out.x1 = out.x1.max(b[2]);
        out.h = out.h.max(b[3] - b[1]);
        prev = Some(i);
    }
    out
}

/// HDDN's rule for hard versus soft breaks: the break before row `k` (`k >= 1`) is hard when the
/// row above ends visibly short of the paragraph's right edge or row `k` starts indented.
fn hard_breaks(parts: &[Part]) -> Vec<bool> {
    let x0 = parts.iter().map(|p| p.x0).fold(f64::INFINITY, f64::min);
    let x1 = parts.iter().map(|p| p.x1).fold(f64::NEG_INFINITY, f64::max);
    let mut hard = vec![true];
    hard.extend(parts.windows(2).map(|w| {
        let (prev, cur) = (&w[0], &w[1]);
        prev.x1 < x1 - prev.h.max(1.0) * 1.5 || cur.x0 > x0 + cur.h.max(1.0) * 0.5
    }));
    hard
}

/// Rows joined by LF at hard breaks and by `soft` (a space) where a line wrapped on its own
/// ([`hard_breaks`]). A hard line's indent becomes leading spaces.
fn original_text(parts: &[Part], space: f64, soft: char) -> String {
    let x0 = parts.iter().map(|p| p.x0).fold(f64::INFINITY, f64::min);
    let lead = |p: &Part| {
        let n = if space > 0.0 {
            ((p.x0 - x0) / space).round()
        } else {
            0.0
        };
        " ".repeat(if p.x0 - x0 >= space && n > 0.0 {
            n as usize
        } else {
            0
        })
    };
    let mut s = format!("{}{}", lead(&parts[0]), parts[0].text);
    for (cur, hard) in parts.iter().zip(hard_breaks(parts)).skip(1) {
        if hard {
            s.push('\n');
            s.push_str(&lead(cur));
        } else {
            s.push(soft);
        }
        s.push_str(&cur.text);
    }
    s
}

/// The style most of `ids` are drawn in; on a tie, the one that comes first.
fn dominant(ids: &[usize], clusters: &[Cluster]) -> u32 {
    let mut counts: Vec<(u32, usize)> = Vec::new();
    for &i in ids {
        let s = clusters[i].style_index;
        match counts.iter_mut().find(|c| c.0 == s) {
            Some(c) => c.1 += 1,
            None => counts.push((s, 1)),
        }
    }
    // `max_by_key` keeps the last maximum, so search from the back.
    counts.iter().rev().max_by_key(|c| c.1).map_or(0, |c| c.0)
}

fn same_style(a: &StyleRun, b: &StyleRun) -> bool {
    a.font_ref == b.font_ref && same_look(a, b)
}

/// Same look on the page: font name (twins of one face in two font objects agree), size, color
/// and flags.
fn same_look(a: &StyleRun, b: &StyleRun) -> bool {
    a.font_name == b.font_name
        && (a.size - b.size).abs() < 0.01
        && a.color
            .iter()
            .zip(&b.color)
            .all(|(x, y)| (x - y).abs() < 0.002)
        && a.flags == b.flags
}

fn style_flags(style: &StyleRun) -> StyleFlags {
    StyleFlags {
        mono: style.flags.mono,
        serif: style.flags.serif,
        bold: style.flags.bold,
        italic: style.flags.italic,
    }
}

/// What the wire says about the font the ladder picked first for `style`.
fn describe(
    style: &StyleRun,
    primary: &Candidate,
    source: Option<&SourceFontFace>,
    registry: &Registry,
    editor_face: bool,
) -> (FontResolution, Option<EditorFace>) {
    match (primary.rule, source) {
        (None, Some(s)) => (
            FontResolution::Embedded {
                font_ref: style.font_ref.clone(),
                name: s.name().to_owned(),
                covers_ascii: s.covers_ascii(),
            },
            s.editor_face().filter(|_| editor_face),
        ),
        (rule, _) => {
            let reg = registry
                .get(primary.face.id())
                .expect("a registered candidate is in the registry");
            (
                FontResolution::Registered {
                    font_id: reg.info().id.clone(),
                    family: reg.info().family.clone(),
                    rule: rule.unwrap_or(FontRule::Flags),
                },
                editor_face.then(|| EditorFace {
                    font_key: format!("reg-{}", reg.info().id),
                    bytes: reg.bytes().to_vec(),
                }),
            )
        }
    }
}

/// UTF-16 offset of the start of line `n` of `text` split at LF.
fn line_offset(text: &str, n: usize) -> u32 {
    let mut at = 0;
    for l in text.split('\n').take(n) {
        at += l.encode_utf16().count() + 1;
    }
    at as u32
}

/// `style` shown as a segment's look.
fn segment_of(
    style: &StyleRun,
    range: (u32, u32),
    resolved: (FontResolution, Option<EditorFace>),
) -> ParagraphSegment {
    ParagraphSegment {
        start: range.0,
        end: range.1,
        font_size: style.size,
        color: style.color,
        bold: style.flags.bold,
        italic: style.flags.italic,
        serif: style.flags.serif,
        mono: style.flags.mono,
        font: resolved.0,
        editor_face: resolved.1,
    }
}

/// A style's font object, its glyphs on the page and the face built from them (ladder rung 2).
#[derive(Default)]
struct SourceFont {
    obj: Option<ObjectId>,
    glyphs: Vec<GlyphItem>,
    face: Option<SourceFontFace>,
}

/// One style segment of a selection (ADR 0012): consecutive hard lines of `originalText` drawn
/// in the same look.
pub struct Segment {
    /// Index of the segment's first line in `originalText` split at LF.
    pub line: usize,
    /// The segment's clusters, as indices into the page's clusters.
    pub clusters: Vec<usize>,
}

/// What `prepare_paragraph` worked out, with what applying an edit needs on top of the wire
/// record: the selected clusters, the source face and the style inputs of the font ladder.
pub struct Prepared {
    pub info: ParagraphInfo,
    /// The selected clusters, in text order.
    pub ids: Vec<ClusterId>,
    /// The style segments, top to bottom; empty when `originalText` is null.
    pub segments: Vec<Segment>,
    /// The paragraph style's font as a ladder rung 2, when it can be reused.
    pub source: Option<SourceFontFace>,
    /// That font's object (the same font as `source`), whether or not it can be reused.
    pub font_obj: Option<ObjectId>,
    pub source_name: String,
    pub flags: StyleFlags,
    /// Every glyph of `font_obj` on the page: the evidence `source` was built from.
    pub glyphs: Vec<GlyphItem>,
}

impl Document {
    /// Everything an editor needs for the paragraph the `rects` select (spec §6.1, §6.2, §6.4).
    /// `font` is a registered font id (`TextEdit.font`). `doc_id` is empty and `revision` is 0:
    /// the session layer that owns them fills them in, as for `extract_text`.
    pub fn prepare_paragraph(
        &self,
        page: usize,
        rects: &[Rect],
        font: Option<&str>,
        editor_face: bool,
        registry: &Registry,
        font_match: &[FontMatchRule],
    ) -> Result<ParagraphInfo, PdfError> {
        Ok(self
            .prepare(page, rects, font, editor_face, registry, font_match)?
            .info)
    }

    /// [`Document::prepare_paragraph`] plus what applying the edit needs.
    pub fn prepare(
        &self,
        page: usize,
        rects: &[Rect],
        font: Option<&str>,
        editor_face: bool,
        registry: &Registry,
        font_match: &[FontMatchRule],
    ) -> Result<Prepared, PdfError> {
        let pick = |l: &TextLayout| select_clusters(l, rects);
        self.prepare_with(page, pick, font, editor_face, registry, font_match)
    }

    /// [`Document::prepare`] for page clusters `ids` (a segment of an earlier selection).
    pub fn prepare_clusters(
        &self,
        page: usize,
        ids: &[usize],
        font: Option<&str>,
        registry: &Registry,
        font_match: &[FontMatchRule],
    ) -> Result<Prepared, PdfError> {
        self.prepare_with(page, |_| ids.to_vec(), font, false, registry, font_match)
    }

    /// The paragraph font's object, its glyphs on the page and the face built from them (rung 2).
    fn source_font(&self, model: &SemanticPage, style: &StyleRun) -> Result<SourceFont, PdfError> {
        let Some((n, g)) = style
            .font_ref
            .split_once(' ')
            .filter(|_| style.font_ref != "0 0")
        else {
            return Ok(SourceFont::default());
        };
        let id = ObjectId {
            num: n.parse().unwrap_or(0),
            generation: g.parse().unwrap_or(0),
        };
        let evidence: Vec<GlyphItem> = model
            .items
            .iter()
            .filter_map(|it| match &it.kind {
                ItemKind::Glyph(g) if g.font == Some(id) => Some(g.clone()),
                _ => None,
            })
            .collect();
        let source = SourceFontFace::new(self, id, &evidence)?;
        Ok(SourceFont {
            obj: Some(id),
            glyphs: evidence,
            face: source,
        })
    }

    fn prepare_with(
        &self,
        page: usize,
        pick: impl FnOnce(&TextLayout) -> Vec<usize>,
        font: Option<&str>,
        editor_face: bool,
        registry: &Registry,
        font_match: &[FontMatchRule],
    ) -> Result<Prepared, PdfError> {
        let model = self.page_model(page)?;
        let layout = TextLayout::build(&model, false)?;
        let clusters = layout.page.clusters.as_deref().unwrap_or_default();
        let styles = &layout.page.styles;
        let sel = pick(&layout);
        if sel.is_empty() {
            return Err(PdfError::new(ErrorCode::EditNoStyle));
        }
        if sel.iter().any(|&i| !upright(&clusters[i].quad)) {
            return Err(PdfError::new(ErrorCode::EditRotatedText));
        }
        let text: Vec<Vec<u16>> = clusters
            .iter()
            .map(|c| {
                let t = layout.page.text.encode_utf16();
                t.skip(c.start as usize)
                    .take((c.end - c.start) as usize)
                    .collect()
            })
            .collect();
        let rtl: String = sel
            .iter()
            .flat_map(|&i| {
                String::from_utf16_lossy(&text[i])
                    .chars()
                    .collect::<Vec<_>>()
            })
            .filter(|&c| is_rtl(c))
            .collect();
        if !rtl.is_empty() {
            return Err(PdfError::with(
                ErrorCode::EditUnsupportedScript,
                ErrorDetails {
                    chars: Some(rtl),
                    ..Default::default()
                },
            ));
        }

        let baseline: Vec<f64> = layout.meta.iter().map(|m| m.baseline).collect();
        // ADR 0012: the paragraph's style is the one most of its clusters are drawn in.
        let style = &styles[dominant(&sel, clusters) as usize];
        let size = style.size;
        let mixed = sel
            .iter()
            .any(|&i| !same_style(style, &styles[clusters[i].style_index as usize]));
        let rows = lines(&sel, clusters, &baseline, styles);
        let sel_bounds = sel.iter().map(|&i| bounds(&clusters[i].quad)).fold(
            [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ],
            |a, b| {
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]
            },
        );
        let [x0, top, x1, y1] = sel_bounds;
        let first_baseline = rows[0].baseline;
        let last_baseline = rows[rows.len() - 1].baseline;
        let mut deltas: Vec<f64> = rows
            .windows(2)
            .map(|w| w[1].baseline - w[0].baseline)
            .filter(|d| *d > 1.0)
            .collect();
        deltas.sort_by(f64::total_cmp);
        let line_step = deltas.get(deltas.len() / 2).copied().unwrap_or(size * 1.2);

        // Page content outside the selection decides the budget; a word with any selected
        // cluster belongs to the paragraph.
        let mut own = vec![false; layout.page.words.len()];
        for &i in &sel {
            own[layout.meta[i].word as usize] = true;
        }
        let others: Vec<[f64; 4]> = layout
            .page
            .words
            .iter()
            .zip(&own)
            .filter(|(w, own)| !**own && !w.invisible)
            .map(|(w, _)| {
                [
                    w.bounds.x,
                    w.bounds.y,
                    w.bounds.x + w.bounds.width,
                    w.bounds.y + w.bounds.height,
                ]
            })
            .collect();
        let visible: Vec<[f64; 4]> = clusters
            .iter()
            .filter(|c| c.flags & INVISIBLE == 0)
            .map(|c| bounds(&c.quad))
            .collect();
        let beside = others.iter().any(|w| w[0] > x1 && w[1] < y1 && w[3] > top);
        let wrap_right = if beside {
            x1
        } else {
            visible.iter().map(|b| b[2]).fold(x1, f64::max)
        };
        let below = others
            .iter()
            .filter(|w| w[1] >= last_baseline && w[2] > x0 && w[0] < wrap_right)
            .map(|w| w[1])
            .fold(f64::INFINITY, f64::min);
        let bottom_limit = if below.is_finite() {
            below
        } else {
            let margin = visible.iter().map(|b| b[1]).fold(f64::INFINITY, f64::min);
            (model.geometry.height - margin).max(y1)
        };

        // Font (§6.4): the ladder's first choice names the paragraph's font; every candidate
        // reserves descent, so the budget holds whichever one a cluster ends up using.
        let SourceFont {
            obj: font_obj,
            glyphs: evidence,
            face: source,
        } = self.source_font(&model, style)?;
        let flags = style_flags(style);
        let ladder = Ladder {
            registry,
            caller: font,
            source: source.as_ref().map(|s| s as &dyn FontFace),
            source_name: &style.font_name,
            flags,
            font_match,
        };
        let candidates = ladder.candidates()?;
        let Some(primary) = candidates.first() else {
            return Err(PdfError::new(ErrorCode::EditNoFont));
        };
        let descent = candidates
            .iter()
            .map(|c| c.face.metrics().descent)
            .fold(0.0, f64::max)
            * size;
        let space = candidates
            .iter()
            .find_map(|c| c.face.shape(" ", size).ok())
            .map_or(size * 0.28, |g| g.iter().map(|g| g.x_advance).sum());
        let (resolution, face) = describe(style, primary, source.as_ref(), registry, editor_face);

        let max_lines = ((bottom_limit + 0.1 - first_baseline - descent) / line_step)
            .floor()
            .max(-1.0) as i64
            + 1;
        let parts: Option<Vec<Part>> = (!sel.iter().any(|&i| clusters[i].flags & UNMAPPED != 0))
            .then(|| {
                rows.iter()
                    .map(|l| line_part(l, clusters, &text, styles))
                    .collect()
            });
        let original = parts.as_deref().map(|p| original_text(p, space, ' '));
        // ADR 0012: a hard break where the rows' look changes (a bold heading over its body)
        // starts a new style segment.
        let mut segments: Vec<Segment> = Vec::new();
        if let Some(p) = parts.as_deref() {
            let mut line = 0;
            let look = |l: &Line| &styles[dominant(&l.clusters, clusters) as usize];
            for (k, hard) in hard_breaks(p).into_iter().enumerate() {
                line += usize::from(hard && k > 0);
                match segments.last_mut() {
                    Some(seg) if !hard || same_look(look(&rows[k - 1]), look(&rows[k])) => {
                        seg.clusters.extend(&rows[k].clusters)
                    }
                    _ => segments.push(Segment {
                        line,
                        clusters: rows[k].clusters.clone(),
                    }),
                }
            }
        }
        // ADR 0011: the box holds the paragraph's own lines as the layout measures them, so text
        // left as it is keeps its breaks where the page draws narrower than the widths the layout
        // reads (TJ corrections, Tc, a fallback font). Every row is laid out as a hard line, in
        // the fonts the page draws it with: never the caller's font, which may be wider (ADR 0012).
        let page_fonts = Ladder {
            caller: None,
            ..ladder
        };
        let measured = parts
            .as_deref()
            .and_then(|p| {
                let params = LayoutParams {
                    font_size: size,
                    wrap_width: f64::INFINITY,
                    max_lines: u32::MAX,
                    line_step,
                    first_baseline,
                    origin_x: x0,
                };
                layout_unbounded(&original_text(p, space, '\n'), &params, &page_fonts).ok()
            })
            .map_or(0.0, |laid| {
                laid.plan
                    .lines
                    .iter()
                    .map(|l| {
                        let w: f64 = l
                            .runs
                            .iter()
                            .flat_map(|r| &r.glyphs)
                            .map(|g| g.x_advance)
                            .sum();
                        l.x - x0 + w
                    })
                    .fold(0.0, f64::max)
            });
        // The wire's segments: the page's own fonts (no caller font), as `page_fonts` measures.
        let mut wire: Vec<ParagraphSegment> = Vec::new();
        if let Some(text) = original.as_deref() {
            let total = text.encode_utf16().count() as u32;
            for (k, seg) in segments.iter().enumerate() {
                let end = segments
                    .get(k + 1)
                    .map_or(total, |n| line_offset(text, n.line));
                let look = &styles[dominant(&seg.clusters, clusters) as usize];
                let face = self.source_font(&model, look)?.face;
                let own = Ladder {
                    source: face.as_ref().map(|s| s as &dyn FontFace),
                    source_name: &look.font_name,
                    flags: style_flags(look),
                    ..page_fonts
                };
                let cands = own.candidates()?;
                let Some(first) = cands.first() else {
                    return Err(PdfError::new(ErrorCode::EditNoFont));
                };
                let resolved = describe(look, first, face.as_ref(), registry, editor_face);
                wire.push(segment_of(
                    look,
                    (line_offset(text, seg.line), end),
                    resolved,
                ));
            }
        }
        let info = ParagraphInfo {
            doc_id: String::new(),
            revision: 0,
            page_index: page as u32,
            bounds: Rect {
                x: x0,
                y: top,
                width: x1 - x0,
                height: y1 - top,
            },
            origin_x: x0,
            top,
            first_baseline,
            font_size: size,
            line_step,
            color: style.color,
            bold: style.flags.bold,
            italic: style.flags.italic,
            serif: style.flags.serif,
            mono: style.flags.mono,
            mixed_style: mixed,
            wrap_width: (wrap_right - x0).max(measured),
            max_height: bottom_limit - top,
            // The lines the paragraph has now are always there to use.
            max_lines: max_lines.max(rows.len() as i64) as u32,
            original_text: original,
            font: resolution,
            editor_face: face,
            segments: wire,
        };
        Ok(Prepared {
            info,
            ids: sel.iter().map(|&i| layout.meta[i].id.clone()).collect(),
            segments,
            source,
            font_obj,
            source_name: style.font_name.clone(),
            flags,
            glyphs: evidence,
        })
    }
}
