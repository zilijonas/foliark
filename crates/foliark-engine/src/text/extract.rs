//! Clusters, styles, lines, words and blocks (spec §3.1 to §3.4).
use crate::interpret::{FontInfo, ItemKind, SemanticPage};
use foliark_model::{
    Cluster, GROUPING, InstancePath, InstanceStep, Matrix, PdfError, Point, Quad, SourceLoc,
    StyleFlags, StyleRun, TextPage, TextStats, Word, WordSource, page_geometry,
};

const INVISIBLE: u32 = 1;
const UNMAPPED: u32 = 2;
const MULTI: u32 = 4;
const OCG_HIDDEN: u32 = 8;

/// A page's text with the geometry selection and search need.
#[derive(Debug, Clone)]
pub struct TextLayout {
    /// Always carries `clusters`; `extract_text` drops them unless asked.
    pub page: TextPage,
    /// Parallel to `page.clusters`: the visual line (page-wide id) and the index into
    /// `page.words` of each cluster.
    pub(super) meta: Vec<ClusterMeta>,
}

impl TextLayout {
    /// Heap held by the per-cluster meta (the ids own their form-path vectors).
    pub(super) fn meta_bytes(&self) -> usize {
        let steps = |m: &ClusterMeta| m.id.path.0.len() * std::mem::size_of::<InstanceStep>();
        self.meta.len() * std::mem::size_of::<ClusterMeta>()
            + self.meta.iter().map(steps).sum::<usize>()
    }
}

#[derive(Debug, Clone)]
pub(super) struct ClusterMeta {
    pub line: u32,
    pub word: u32,
    pub id: ClusterId,
}

/// Names one shown glyph of the input file, so it can be found again after a reopen: the page,
/// the Form instances it is drawn through, the operator, and the code's index inside that
/// operator (`op_glyph_index`). One glyph is one cluster (spec §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClusterId {
    pub page: u32,
    pub path: InstancePath,
    pub loc: SourceLoc,
    pub glyph: u32,
}

/// One shown glyph in display space, before grouping.
struct Glyph {
    text: String,
    quad: Quad,
    /// Bounds in unrotated crop space, for the §3.4 sort.
    unrot: [f64; 4],
    origin: Point,
    /// Writing direction (unit) and the pen position after the advance.
    dir: Point,
    end: Point,
    size: f64,
    style: u32,
    flags: u32,
    whitespace: bool,
    id: ClusterId,
}

struct Line {
    glyphs: Vec<usize>,
    /// Extent of the quads along `dir`.
    span: (f64, f64),
}

struct WordSpan {
    glyphs: Vec<usize>,
    block: u32,
    line_in_block: u32,
    line: u32,
    index_in_line: u32,
}

fn sub(a: Point, b: Point) -> Point {
    Point {
        x: a.x - b.x,
        y: a.y - b.y,
    }
}

fn dot(a: Point, b: Point) -> f64 {
    a.x * b.x + a.y * b.y
}

fn cross(a: Point, b: Point) -> f64 {
    a.x * b.y - a.y * b.x
}

fn same_direction(a: Point, b: Point) -> bool {
    cross(a, b).abs() < 0.01 && dot(a, b) > 0.0
}

pub(super) fn bounds(quads: impl Iterator<Item = Quad>) -> [f64; 4] {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in quads.flat_map(|q| q.0) {
        b = [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)];
    }
    b
}

impl TextLayout {
    pub fn build(model: &SemanticPage, include_ocg_hidden: bool) -> Result<Self, PdfError> {
        let g = &model.geometry;
        let unrot =
            page_geometry(g.index, g.media_box, Some(g.crop_box), 0, g.user_unit)?.user_to_display;
        let mut styles: Vec<StyleRun> = Vec::new();
        let mut glyphs: Vec<Glyph> = Vec::new();
        let mut image_coverage = 0.0_f64;
        let on_page = |b: [f64; 4]| b[0] < g.width && b[2] > 0.0 && b[1] < g.height && b[3] > 0.0;
        for item in &model.items {
            if item.ocg_hidden && !include_ocg_hidden {
                continue;
            }
            match &item.kind {
                ItemKind::Image(img) if !item.ocg_hidden => {
                    let unit = Quad(
                        [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
                            .map(|(x, y)| Point { x, y }),
                    );
                    let b = bounds([unit.map(img.ctm).map(g.user_to_display)].into_iter());
                    let w = (b[2].min(g.width) - b[0].max(0.0)).max(0.0);
                    let h = (b[3].min(g.height) - b[1].max(0.0)).max(0.0);
                    image_coverage = image_coverage.max(w * h / (g.width * g.height));
                }
                ItemKind::Glyph(glyph) => {
                    let quad = glyph.quad_user.map(g.user_to_display);
                    if !on_page(bounds([quad].into_iter())) {
                        continue;
                    }
                    let m = glyph.trm.then(g.user_to_display);
                    let e = if glyph.vertical {
                        (0.0, glyph.advance_em.signum())
                    } else {
                        (1.0, 0.0)
                    };
                    let origin = m.apply(Point { x: 0.0, y: 0.0 });
                    let axis = sub(m.apply(Point { x: e.0, y: e.1 }), origin);
                    let len = axis.x.hypot(axis.y);
                    let dir = if len > 0.0 {
                        Point {
                            x: axis.x / len,
                            y: axis.y / len,
                        }
                    } else {
                        Point { x: 1.0, y: 0.0 }
                    };
                    let end = if glyph.vertical {
                        m.apply(Point {
                            x: 0.0,
                            y: glyph.advance_em,
                        })
                    } else {
                        m.apply(Point {
                            x: glyph.advance_em,
                            y: 0.0,
                        })
                    };
                    let (text, mut flags) = cluster_text(glyph.unicode.as_deref());
                    let whitespace = flags & UNMAPPED == 0 && text.chars().all(char::is_whitespace);
                    if matches!(glyph.render_mode, 3 | 7) {
                        flags |= INVISIBLE;
                    }
                    if item.ocg_hidden {
                        flags |= OCG_HIDDEN;
                    }
                    let info = glyph.font.and_then(|f| model.fonts.get(&f));
                    let style = style_of(&mut styles, glyph, info, m);
                    let unrot_quad = glyph.quad_user.map(unrot);
                    glyphs.push(Glyph {
                        text,
                        quad,
                        unrot: bounds([unrot_quad].into_iter()),
                        origin,
                        dir,
                        end,
                        size: m.0[2].hypot(m.0[3]).max(1e-6),
                        style,
                        flags,
                        whitespace,
                        id: ClusterId {
                            page: model.index as u32,
                            path: item.path.clone(),
                            loc: item.loc,
                            glyph: glyph.op_glyph_index,
                        },
                    });
                }
                _ => {}
            }
        }
        let spans = group(&glyphs);
        Ok(assemble(model, glyphs, spans, styles, image_coverage))
    }
}

/// The cluster's string and flags (§3.1): none, empty or a control character is U+FFFD.
fn cluster_text(unicode: Option<&str>) -> (String, u32) {
    match unicode {
        Some(s) if !s.is_empty() && !s.chars().any(|c| c.is_control() && !c.is_whitespace()) => (
            s.to_owned(),
            if s.chars().nth(1).is_some() { MULTI } else { 0 },
        ),
        _ => ("\u{FFFD}".to_owned(), UNMAPPED),
    }
}

fn style_of(
    styles: &mut Vec<StyleRun>,
    glyph: &crate::interpret::GlyphItem,
    info: Option<&FontInfo>,
    m: Matrix,
) -> u32 {
    let [r, g, b, _] = glyph.fill.unwrap_or([0, 0, 0, 255]);
    let run = StyleRun {
        font_name: info.map(|f| f.name.clone()).unwrap_or_default(),
        font_ref: glyph.font.map_or_else(
            || "0 0".to_owned(),
            |f| format!("{} {}", f.num, f.generation),
        ),
        size: m.0[2].hypot(m.0[3]),
        color: [r, g, b].map(|c| f64::from(c) / 255.0),
        render_mode: glyph.render_mode,
        flags: StyleFlags {
            bold: info.is_some_and(|f| f.bold),
            italic: info.is_some_and(|f| f.italic),
            serif: info.is_some_and(|f| f.serif),
            mono: info.is_some_and(|f| f.mono),
        },
    };
    let i = styles.iter().position(|s| *s == run).unwrap_or_else(|| {
        styles.push(run);
        styles.len() - 1
    });
    i as u32
}

/// §3.3: glyphs in content order to lines, blocks and words.
fn group(glyphs: &[Glyph]) -> Vec<WordSpan> {
    let k = GROUPING;
    let mut lines: Vec<Line> = Vec::new();
    for (i, c) in glyphs.iter().enumerate() {
        let continues = lines.last().is_some_and(|l| {
            let p = &glyphs[*l.glyphs.last().expect("line has glyphs")];
            let em = c.size.max(p.size);
            let perp = Point {
                x: -p.dir.y,
                y: p.dir.x,
            };
            let gap = dot(sub(c.origin, p.end), p.dir) / em;
            same_direction(c.dir, p.dir)
                && dot(sub(c.origin, p.origin), perp).abs() <= k.same_line_perp * em
                && (-k.new_line_back..=k.new_line_fwd).contains(&gap)
        });
        let extent = quad_extent(&c.quad, c.dir);
        match lines.last_mut() {
            Some(l) if continues => {
                l.glyphs.push(i);
                l.span = (l.span.0.min(extent.0), l.span.1.max(extent.1));
            }
            _ => lines.push(Line {
                glyphs: vec![i],
                span: extent,
            }),
        }
    }
    let mut words = Vec::new();
    let (mut block, mut line_in_block) = (0_u32, 0_u32);
    for (n, line) in lines.iter().enumerate() {
        let first = &glyphs[line.glyphs[0]];
        if n > 0 {
            let prev = &lines[n - 1];
            let before = &glyphs[prev.glyphs[0]];
            let em = first.size.max(before.size);
            let down = Point {
                x: -before.dir.y,
                y: before.dir.x,
            };
            let advance = dot(sub(first.origin, before.origin), down);
            let overlap = line.span.0 <= prev.span.1 && prev.span.0 <= line.span.1;
            if same_direction(first.dir, before.dir)
                && advance > 0.0
                && advance <= k.block_gap * em
                && overlap
            {
                line_in_block += 1;
            } else {
                block += 1;
                line_in_block = 0;
            }
        }
        let mut open: Vec<usize> = Vec::new();
        let mut index_in_line = 0;
        let mut flush = |open: &mut Vec<usize>| {
            if !open.is_empty() {
                words.push(WordSpan {
                    glyphs: std::mem::take(open),
                    block,
                    line_in_block,
                    line: n as u32,
                    index_in_line,
                });
                index_in_line += 1;
            }
        };
        for (j, &gi) in line.glyphs.iter().enumerate() {
            let c = &glyphs[gi];
            if c.whitespace {
                flush(&mut open);
                continue;
            }
            if j > 0 && !open.is_empty() {
                let p = &glyphs[line.glyphs[j - 1]];
                let gap = dot(sub(c.origin, p.end), p.dir) / c.size.max(p.size);
                if gap >= k.word_gap {
                    flush(&mut open);
                }
            }
            open.push(gi);
        }
        flush(&mut open);
    }
    words
}

fn quad_extent(q: &Quad, dir: Point) -> (f64, f64) {
    let proj = q.0.map(|p| dot(p, dir));
    (
        proj.into_iter().fold(f64::INFINITY, f64::min),
        proj.into_iter().fold(f64::NEG_INFINITY, f64::max),
    )
}

/// §3.4: sort, join, offsets.
fn assemble(
    model: &SemanticPage,
    glyphs: Vec<Glyph>,
    mut spans: Vec<WordSpan>,
    styles: Vec<StyleRun>,
    image_coverage: f64,
) -> TextLayout {
    // Float noise must not reorder words on one baseline: keys are rounded to 1e-4 pt.
    let key = |w: &WordSpan| {
        let u = bounds_of(&glyphs, &w.glyphs, |g| g.unrot);
        ((u[3] * 1e4).round(), u[0])
    };
    let mut keyed: Vec<_> = spans.drain(..).map(|w| (key(&w), w)).collect();
    keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("finite"));
    let (mut text, mut offset) = (String::new(), 0_u32);
    let (mut words, mut clusters, mut meta) = (Vec::new(), Vec::new(), Vec::new());
    let mut stats = TextStats {
        visible_clusters: 0,
        invisible_clusters: 0,
        unmapped_glyphs: 0,
        image_coverage,
    };
    for (n, (_, w)) in keyed.iter().enumerate() {
        if n > 0 {
            text.push(' ');
            offset += 1;
        }
        let start = offset;
        for &gi in &w.glyphs {
            let g = &glyphs[gi];
            let len = g.text.encode_utf16().count() as u32;
            text.push_str(&g.text);
            clusters.push(Cluster {
                start: offset,
                end: offset + len,
                quad: g.quad,
                style_index: g.style,
                glyphs: 1,
                flags: g.flags,
            });
            meta.push(ClusterMeta {
                line: w.line,
                word: n as u32,
                id: g.id.clone(),
            });
            offset += len;
            stats.unmapped_glyphs += u32::from(g.flags & UNMAPPED != 0);
            if g.flags & INVISIBLE != 0 {
                stats.invisible_clusters += 1;
            } else {
                stats.visible_clusters += 1;
            }
        }
        let b = bounds_of(&glyphs, &w.glyphs, |g| bounds([g.quad].into_iter()));
        let flags = w.glyphs.iter().map(|&i| glyphs[i].flags);
        words.push(Word {
            text: w.glyphs.iter().map(|&i| glyphs[i].text.as_str()).collect(),
            start,
            end: offset,
            bounds: foliark_model::Rect {
                x: b[0],
                y: b[1],
                width: b[2] - b[0],
                height: b[3] - b[1],
            },
            block_index: w.block,
            line_index: w.line_in_block,
            word_index: w.index_in_line,
            style_index: glyphs[w.glyphs[0]].style,
            invisible: flags.clone().all(|f| f & INVISIBLE != 0),
            ocg_hidden: flags.clone().any(|f| f & OCG_HIDDEN != 0).then_some(true),
            source: WordSource::Native,
        });
    }
    TextLayout {
        page: TextPage {
            doc_id: String::new(),
            revision: 0,
            page_index: model.index as u32,
            text,
            words,
            styles,
            clusters: Some(clusters),
            stats,
        },
        meta,
    }
}

fn bounds_of(glyphs: &[Glyph], idx: &[usize], f: impl Fn(&Glyph) -> [f64; 4]) -> [f64; 4] {
    idx.iter().map(|&i| f(&glyphs[i])).fold(
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
    )
}
