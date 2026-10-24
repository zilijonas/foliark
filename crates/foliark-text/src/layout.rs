//! Paragraph input rules (spec §6.1), the font ladder (§6.4) and greedy breaking (§6.3).
use crate::Fnv;
use crate::registry::Registry;
use foliark_model::{
    EditLayout, ErrorCode, ErrorDetails, FontMatchRule, FontRule, LayoutRun, PdfError, RunSource,
};
use std::ops::Range;
use unicode_normalization::{UnicodeNormalization, char::canonical_combining_class};

/// One positioned glyph. For registered fonts `gid` is a glyph id; a source-font face may use its
/// character code. `cluster` is the UTF-16 range of input text the glyph stands for (a ligature
/// spans several characters). `FontFace::shape` returns it relative to the shaped text; a
/// `LayoutPlan` holds it relative to the normalized paragraph.
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    pub gid: u32,
    pub x_advance: f64,
    pub cluster: Range<u32>,
}

/// Fractions of the font size; `descent` is a magnitude.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceMetrics {
    pub ascent: f64,
    pub descent: f64,
}

/// What the ladder needs from a font. A PDF simple or CID font with `/Widths` implements it
/// without shaping: `covers` = invertible code map, `shape` = one glyph per character.
pub trait FontFace {
    /// Registered font id, or the source font's object reference.
    fn id(&self) -> &str;
    fn source(&self) -> RunSource;
    /// True iff every character of the (grapheme) cluster has a glyph.
    fn covers(&self, cluster: &str) -> bool;
    /// Glyphs for `text` (one font run, LTR) at `size` points, cluster ranges relative to `text`.
    fn shape(&self, text: &str, size: f64) -> Result<Vec<Glyph>, PdfError>;
    fn metrics(&self) -> FaceMetrics;
}

/// HDDN's table (spec §6.4); caller rules are tried first.
pub const DEFAULT_FONT_MATCH: &[(&str, &str)] = &[
    ("calibri", "Carlito"),
    ("cambria", "Caladea"),
    ("georgia", "Gelasio"),
    ("times", "Liberation Serif"),
    ("roman", "Liberation Serif"),
    ("minion", "Liberation Serif"),
    ("garamond", "Liberation Serif"),
    ("courier", "Liberation Mono"),
    ("consol", "Liberation Mono"),
    ("mono", "Liberation Mono"),
    ("arial", "Liberation Sans"),
    ("helvetica", "Liberation Sans"),
    ("verdana", "Liberation Sans"),
    ("tahoma", "Liberation Sans"),
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StyleFlags {
    pub mono: bool,
    pub serif: bool,
    pub bold: bool,
    pub italic: bool,
}

/// Everything that decides which font a cluster gets.
#[derive(Clone, Copy)]
pub struct Ladder<'a> {
    pub registry: &'a Registry,
    /// `TextEdit.font`: a registered font id (rung 1).
    pub caller: Option<&'a str>,
    /// Rung 2, supplied by the PDF side (NEW-48).
    pub source: Option<&'a dyn FontFace>,
    /// Source font name, e.g. `ABCDEF+Calibri-Bold`; the subset prefix and case are ignored.
    pub source_name: &'a str,
    pub flags: StyleFlags,
    pub font_match: &'a [FontMatchRule],
}

pub struct Candidate<'a> {
    pub face: &'a dyn FontFace,
    /// `None` = the source font itself.
    pub rule: Option<FontRule>,
}

impl<'a> Ladder<'a> {
    /// Rungs 1 to 4 in order. A matching rule whose family is not registered is skipped.
    pub fn candidates(&self) -> Result<Vec<Candidate<'a>>, PdfError> {
        let reg = self.registry;
        let f = self.flags;
        let mut out = Vec::new();
        if let Some(id) = self.caller {
            let face = reg
                .get(id)
                .ok_or_else(|| PdfError::new(ErrorCode::InvalidArgument))?;
            out.push(Candidate {
                face,
                rule: Some(FontRule::Caller),
            });
        }
        if let Some(face) = self.source {
            out.push(Candidate { face, rule: None });
        }
        let name = {
            let n = self.source_name;
            let n = match n.as_bytes().get(6) {
                Some(b'+') if n[..6].bytes().all(|b| b.is_ascii_uppercase()) => &n[7..],
                _ => n,
            };
            n.to_lowercase()
        };
        let rules = self
            .font_match
            .iter()
            .map(|r| (r.pattern.as_str(), r.family.as_str()));
        let matched = rules
            .chain(DEFAULT_FONT_MATCH.iter().copied())
            .filter(|(p, _)| !p.is_empty() && name.contains(p))
            .find_map(|(_, fam)| reg.lookup(fam, f.bold, f.italic));
        if let Some(face) = matched {
            out.push(Candidate {
                face,
                rule: Some(FontRule::NameMatch),
            });
        }
        let family = match (f.mono, f.serif) {
            (true, _) => "Liberation Mono",
            (false, true) => "Liberation Serif",
            _ => "Liberation Sans",
        };
        if let Some(face) = reg.lookup(family, f.bold, f.italic) {
            out.push(Candidate {
                face,
                rule: Some(FontRule::Flags),
            });
        }
        Ok(out)
    }
}

/// Fixed geometry, display points (spec §6.2 values). The font size is never changed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutParams {
    pub font_size: f64,
    pub wrap_width: f64,
    pub max_lines: u32,
    pub line_step: f64,
    pub first_baseline: f64,
    pub origin_x: f64,
}

/// One style run of the text to lay out (ADR 0013): from byte `start` of the normalized text to
/// the next run's start, drawn with its own ladder, size and colour.
#[derive(Clone, Copy)]
pub struct TextRun<'a> {
    pub start: usize,
    pub ladder: Ladder<'a>,
    pub font_size: f64,
    pub color: [f64; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanRun {
    pub font: String,
    pub source: RunSource,
    /// `None` = the source font.
    pub rule: Option<FontRule>,
    pub size: f64,
    /// Fill colour, RGB 0..1.
    pub color: [f64; 3],
    pub glyphs: Vec<Glyph>,
    pub clusters: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanLine {
    pub baseline_y: f64,
    /// Origin of the first glyph: `origin_x` plus the leading indent on a hard line's first line.
    pub x: f64,
    pub runs: Vec<PlanRun>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutPlan {
    pub lines: Vec<PlanLine>,
    /// Some cluster is not drawn with the source font.
    pub approximate: bool,
    /// Hash of every input and the registry version.
    pub fingerprint: String,
}

impl LayoutPlan {
    /// The `ChangeResult.layouts` entry; runs are totalled per font in first-use order.
    pub fn edit_layout(&self, id: &str) -> EditLayout {
        let mut runs: Vec<LayoutRun> = Vec::new();
        for r in self.lines.iter().flat_map(|l| &l.runs) {
            match runs.iter_mut().find(|x| x.font == r.font) {
                Some(x) => x.clusters += r.clusters,
                None => runs.push(LayoutRun {
                    source: r.source,
                    font: r.font.clone(),
                    clusters: r.clusters,
                }),
            }
        }
        EditLayout {
            id: id.into(),
            lines: self.lines.len() as u32,
            approximate: self.approximate,
            fingerprint: self.fingerprint.clone(),
            runs,
        }
    }
}

fn refuse(code: ErrorCode, d: ErrorDetails) -> PdfError {
    PdfError::with(code, d)
}

fn allowed(c: char) -> bool {
    matches!(c as u32,
        0x0A | 0x20..=0x7E | 0xA0..=0x36F | 0x1E00..=0x1EFF
        | 0x2000..=0x200D | 0x2010..=0x2029 | 0x202F..=0x2065 | 0x206A..=0x206F
        | 0x20A0..=0x20CF | 0x2100..=0x214F | 0x2C60..=0x2C7F | 0xA720..=0xA7FF | 0xFB00..=0xFB06)
}

/// §6.1: CRLF/CR to LF; per line keep the leading-space indent, collapse interior spaces,
/// drop trailing spaces; NFC; then refuse anything outside the allowed ranges.
pub fn normalize(text: &str) -> Result<String, PdfError> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<String> = text
        .split('\n')
        .map(|l| {
            let body = l.trim_start_matches(' ');
            let indent = &l[..l.len() - body.len()];
            let words: Vec<&str> = body.split(' ').filter(|w| !w.is_empty()).collect();
            if words.is_empty() {
                String::new()
            } else {
                format!("{indent}{}", words.join(" "))
            }
        })
        .collect();
    let out: String = lines.join("\n").nfc().collect();
    let mut bad = String::new();
    for c in out.chars().filter(|&c| !allowed(c)) {
        if !bad.contains(c) {
            bad.push(c);
        }
    }
    if bad.is_empty() {
        Ok(out)
    } else {
        Err(refuse(
            ErrorCode::EditUnsupportedScript,
            ErrorDetails {
                chars: Some(bad),
                ..Default::default()
            },
        ))
    }
}

/// One grapheme-ish cluster: a base character plus the combining marks that follow it.
struct Cluster {
    bytes: Range<usize>,
    start16: u32,
    cand: usize,
    /// Drawn as its canonical decomposition: the font has the base and marks, not the letter.
    nfd: bool,
}

struct Piece {
    cand: usize,
    glyphs: Vec<Glyph>,
    clusters: u32,
}

struct Segment {
    word: Vec<Piece>,
    word_w: f64,
    space: Option<(Piece, f64)>,
}

fn width(pieces: &[Piece]) -> f64 {
    pieces
        .iter()
        .flat_map(|p| &p.glyphs)
        .map(|g| g.x_advance)
        .sum()
}

struct Shaper<'a> {
    text: &'a str,
    /// Per candidate: its run's size.
    sizes: &'a [f64],
    cands: &'a [Candidate<'a>],
}

impl Shaper<'_> {
    /// Shapes consecutive same-font clusters as one run each; a decomposed cluster is a run of
    /// its own whose glyphs all stand for the whole cluster.
    fn pieces(&self, cl: &[Cluster]) -> Result<Vec<Piece>, PdfError> {
        let mut out = Vec::new();
        for run in cl.chunk_by(|a, b| a.cand == b.cand && !a.nfd && !b.nfd) {
            let (a, b) = (run[0].bytes.start, run[run.len() - 1].bytes.end);
            let (face, t) = (self.cands[run[0].cand].face, &self.text[a..b]);
            let size = self.sizes[run[0].cand];
            let mut glyphs = if run[0].nfd {
                let mut g = face.shape(&t.nfd().collect::<String>(), size)?;
                let n16 = t.encode_utf16().count() as u32;
                g.iter_mut().for_each(|g| g.cluster = 0..n16);
                g
            } else {
                face.shape(t, size)?
            };
            for g in &mut glyphs {
                g.cluster = g.cluster.start + run[0].start16..g.cluster.end + run[0].start16;
            }
            out.push(Piece {
                cand: run[0].cand,
                glyphs,
                clusters: run.len() as u32,
            });
        }
        Ok(out)
    }
}

/// Splits one hard line's clusters into words: after U+0020 (consumed) and after `-`/U+2010.
fn segments(sh: &Shaper, body: &[Cluster]) -> Result<Vec<Segment>, PdfError> {
    let mut segs: Vec<Segment> = Vec::new();
    let mut start = 0;
    for (i, c) in body.iter().enumerate() {
        let t = &sh.text[c.bytes.clone()];
        let (space, hyphen) = (t == " ", t == "-" || t == "\u{2010}");
        if !(space || hyphen || i + 1 == body.len()) {
            continue;
        }
        let word = sh.pieces(&body[start..if space { i } else { i + 1 }])?;
        let space = space
            .then(|| sh.pieces(&body[i..=i]))
            .transpose()?
            .map(|p| {
                let w = width(&p);
                (p.into_iter().next().expect("one cluster, one piece"), w)
            });
        segs.push(Segment {
            word_w: width(&word),
            word,
            space,
        });
        start = i + 1;
    }
    Ok(segs)
}

/// A laid-out paragraph before the budget check: `plan` holds every line, even past `max_lines`.
#[derive(Debug, Clone, PartialEq)]
pub struct Laid {
    pub plan: LayoutPlan,
    pub max_lines: u32,
    /// A word is wider than its line, so no number of lines fits it.
    pub too_wide: bool,
}

impl Laid {
    /// Lines the text takes; one more than `max_lines` at least when a word is too wide.
    pub fn needed_lines(&self) -> u32 {
        let n = self.plan.lines.len() as u32;
        if self.too_wide {
            n.max(self.max_lines + 1)
        } else {
            n
        }
    }

    pub fn fits(&self) -> bool {
        !self.too_wide && self.needed_lines() <= self.max_lines
    }

    /// The plan, or `EDIT_NO_FIT` with the counts (§6.3).
    pub fn fit(self) -> Result<LayoutPlan, PdfError> {
        if self.fits() {
            return Ok(self.plan);
        }
        Err(refuse(
            ErrorCode::EditNoFit,
            ErrorDetails {
                needed_lines: Some(self.needed_lines()),
                max_lines: Some(self.max_lines),
                ..Default::default()
            },
        ))
    }
}

/// §6.3 greedy layout of `text` at a fixed size; `EDIT_NO_FIT` when it does not fit.
pub fn layout_paragraph(
    text: &str,
    p: &LayoutParams,
    ladder: &Ladder,
) -> Result<LayoutPlan, PdfError> {
    layout_unbounded(text, p, ladder)?.fit()
}

/// [`layout_paragraph`] without the budget check, for previews: the same lines and fingerprint
/// the commit would have, and whether they fit. Missing glyphs and bad text still refuse.
pub fn layout_unbounded(text: &str, p: &LayoutParams, ladder: &Ladder) -> Result<Laid, PdfError> {
    let run = TextRun {
        start: 0,
        ladder: *ladder,
        font_size: p.font_size,
        color: [0.0; 3],
    };
    layout_runs(text, p, &[run])
}

/// [`layout_unbounded`] with style runs (ADR 0013): each cluster takes the ladder, size and
/// colour of the run its first byte is in. Run starts index the normalized text, so text with
/// more than one run must already be normalized. Breaking and the budget are the paragraph's.
pub fn layout_runs(input: &str, p: &LayoutParams, runs: &[TextRun]) -> Result<Laid, PdfError> {
    const EPS: f64 = 1e-6;
    let text = normalize(input)?;
    if runs.first().is_none_or(|r| r.start != 0) || (runs.len() > 1 && text != input) {
        return Err(PdfError::new(ErrorCode::Internal));
    }
    // Every run's candidates in one list; `of_run[k]` is run k's slice of it.
    let (mut cands, mut sizes, mut colors, mut of_run) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for r in runs {
        let c = r.ladder.candidates()?;
        let at = cands.len();
        sizes.extend(std::iter::repeat_n(r.font_size, c.len()));
        colors.extend(std::iter::repeat_n(r.color, c.len()));
        cands.extend(c);
        of_run.push(at..cands.len());
    }

    // Resolve every cluster through the ladder first, so one refusal lists all missing characters.
    let mut hard: Vec<(Vec<Cluster>, Vec<Cluster>)> = Vec::new(); // (indent, body)
    let (mut missing, mut approximate) = (String::new(), false);
    let (mut off, mut off16) = (0, 0u32);
    for line in if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').collect()
    } {
        let mut cl: Vec<Cluster> = Vec::new();
        for (i, c) in line.char_indices() {
            let attach = canonical_combining_class(c) != 0 && !cl.is_empty();
            if attach {
                let last = cl.last_mut().expect("checked non-empty");
                last.bytes.end = i + c.len_utf8();
            } else {
                cl.push(Cluster {
                    bytes: i..i + c.len_utf8(),
                    start16: 0,
                    cand: 0,
                    nfd: false,
                });
            }
        }
        let mut at16 = off16;
        for c in &mut cl {
            let t = &line[c.bytes.clone()];
            c.start16 = at16;
            at16 += t.encode_utf16().count() as u32;
            // NFC first, then the decomposition (a subset may hold `a` and U+0328 but not `ą`).
            let nfd: String = t.nfd().collect();
            let own = &of_run[runs.partition_point(|r| r.start <= off + c.bytes.start) - 1];
            let found = cands[own.clone()]
                .iter()
                .zip(own.clone())
                .find_map(|(x, k)| {
                    if x.face.covers(t) {
                        Some((k, false))
                    } else {
                        (nfd != t && x.face.covers(&nfd)).then_some((k, true))
                    }
                });
            match found {
                Some((k, nfd)) => {
                    (c.cand, c.nfd) = (k, nfd);
                    approximate |= cands[k].rule.is_some();
                }
                None if !missing.contains(t) => missing.push_str(t),
                None => {}
            }
            c.bytes = c.bytes.start + off..c.bytes.end + off;
        }
        let n_indent = line.len() - line.trim_start_matches(' ').len();
        let body = cl.split_off(n_indent);
        hard.push((cl, body));
        off += line.len() + 1;
        off16 = at16 + 1;
    }
    if !missing.is_empty() {
        return Err(refuse(
            ErrorCode::EditNoFont,
            ErrorDetails {
                chars: Some(missing),
                ..Default::default()
            },
        ));
    }

    let sh = Shaper {
        text: &text,
        sizes: &sizes,
        cands: &cands,
    };
    let mut lines: Vec<(f64, Vec<Piece>)> = Vec::new();
    let mut too_wide = false;
    for (indent, body) in &hard {
        let indent_w = width(&sh.pieces(indent)?);
        let segs = segments(&sh, body)?;
        if segs.is_empty() {
            lines.push((p.origin_x, Vec::new()));
            continue;
        }
        let (mut x, mut avail) = (p.origin_x + indent_w, p.wrap_width - indent_w);
        let (mut cur, mut cur_w): (Vec<Piece>, f64) = (Vec::new(), 0.0);
        let mut pending: Option<(Piece, f64)> = None;
        for seg in segs {
            let gap = pending.as_ref().map_or(0.0, |s| s.1);
            if !cur.is_empty() && cur_w + gap + seg.word_w > avail + EPS {
                lines.push((x, std::mem::take(&mut cur)));
                (x, avail, cur_w, pending) = (p.origin_x, p.wrap_width, 0.0, None);
            }
            if seg.word_w > avail + EPS {
                too_wide = true;
            }
            if let Some((sp, w)) = pending.take()
                && !cur.is_empty()
            {
                cur.push(sp);
                cur_w += w;
            }
            cur_w += seg.word_w;
            cur.extend(seg.word);
            pending = seg.space;
        }
        lines.push((x, cur));
    }

    let plan_lines = lines
        .into_iter()
        .enumerate()
        .map(|(k, (x, pieces))| {
            let mut out: Vec<PlanRun> = Vec::new();
            for pc in pieces {
                let (c, size, color) = (&cands[pc.cand], sizes[pc.cand], colors[pc.cand]);
                match out.last_mut() {
                    Some(r)
                        if r.font == c.face.id()
                            && r.rule == c.rule
                            && r.size == size
                            && r.color == color =>
                    {
                        r.glyphs.extend(pc.glyphs);
                        r.clusters += pc.clusters;
                    }
                    _ => out.push(PlanRun {
                        font: c.face.id().into(),
                        source: c.face.source(),
                        rule: c.rule,
                        size,
                        color,
                        glyphs: pc.glyphs,
                        clusters: pc.clusters,
                    }),
                }
            }
            PlanLine {
                baseline_y: p.first_baseline + k as f64 * p.line_step,
                x,
                runs: out,
            }
        })
        .collect();

    let mut h = Fnv::new();
    h.field(text.as_bytes());
    for v in [
        p.font_size,
        p.wrap_width,
        p.line_step,
        p.first_baseline,
        p.origin_x,
    ] {
        h.bytes(&v.to_bits().to_le_bytes());
    }
    h.bytes(&p.max_lines.to_le_bytes())
        .bytes(&runs[0].ladder.registry.version().to_le_bytes());
    for r in runs {
        let l = &r.ladder;
        h.bytes(&(r.start as u64).to_le_bytes())
            .bytes(&r.font_size.to_bits().to_le_bytes());
        for c in r.color {
            h.bytes(&c.to_bits().to_le_bytes());
        }
        h.field(l.caller.unwrap_or("").as_bytes())
            .field(l.source.map_or("", |s| s.id()).as_bytes())
            .field(l.source_name.as_bytes())
            .bytes(&[
                l.flags.mono as u8,
                l.flags.serif as u8,
                l.flags.bold as u8,
                l.flags.italic as u8,
            ]);
    }
    for r in runs[0].ladder.font_match {
        h.field(r.pattern.as_bytes()).field(r.family.as_bytes());
    }
    Ok(Laid {
        plan: LayoutPlan {
            lines: plan_lines,
            approximate,
            fingerprint: h.hex(),
        },
        max_lines: p.max_lines,
        too_wide,
    })
}
