//! Text removal (spec §5.1, §5.6), executed through the writer.
//!
//! Every show operator that draws a planned cluster is rewritten as one `TJ` in which each
//! removed code is a displacement of exactly its advance, so every surviving glyph of the same
//! and of later operators stays where it was (Tc, Tw and Tz cancel: the advance the model
//! reports is `w0 + (Tc + Tw) / Tfs` in em, and `TJ` shifts by `-n/1000` em). Then each font
//! that lost a glyph gets its ToUnicode rebuilt from what survives anywhere in the document and,
//! for TrueType, the outlines nothing draws any more emptied.
//!
//! Limits, all typed or reported, none silent:
//! - A cluster drawn through a Form (or a content stream) that something else draws too is
//!   rewritten in a clone of that occurrence's chain (`forms`); the other occurrences keep
//!   the original. A Form drawn exactly once in the document is rewritten in place.
//! - A removed glyph of a font whose dictionary is a direct object cannot get a rebuilt
//!   ToUnicode: `FONT_UNSUPPORTED`. Type0 fonts other than Identity-H/V likewise (their code
//!   width is unknown).
//! - CFF / Type 1 programs are kept and Type3 CharProcs are not dropped: both are listed in
//!   [`TextReport::fonts_with_glyph_inventory`] for `FONT_GLYPH_INVENTORY_RETAINED`.
//! - Two font dictionaries sharing one TrueType program: the later one's survivors win.
//!
//! On `Err` the writer may hold part of the work: drop it.
use super::forms::Targets;
use super::plan::{PlannedCluster, RedactionPlan};
use crate::Document;
use crate::limits::{self, Budget};
use crate::writer::fonts::{self, Program};
use crate::writer::{Writer, string_token};
use foliark_model::{ErrorCode, ObjectId, PdfError};
use hayro::hayro_syntax::content::UntypedIter;
use hayro::hayro_syntax::object::{Dict, Name, Object, ObjectIdentifier, Stream};
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextReport {
    pub clusters_removed: u32,
    /// Of those, OCR-layer clusters (render mode 3 or 7).
    pub invisible_clusters_removed: u32,
    pub operators_rewritten: u32,
    pub fonts_touched: u32,
    /// Touched fonts whose glyph programs were kept: (font object, BaseFont).
    pub fonts_with_glyph_inventory: Vec<(ObjectId, String)>,
}

pub(super) fn at(id: ObjectId) -> ObjectIdentifier {
    ObjectIdentifier::new(id.num as i32, i32::from(id.generation))
}

/// Remove the clusters of `plan` from `w`'s output. `doc` is the working copy the plan was
/// built from (and `w` writes).
pub fn redact_text(
    doc: &Document,
    plan: &RedactionPlan,
    w: &mut Writer<'_>,
) -> Result<TextReport, PdfError> {
    let mut report = TextReport::default();
    // Operators to rewrite, in file order, and what makes each of them safe to rewrite.
    // Keyed by the stream the operator is written into (a clone for a shared occurrence).
    let targets = Targets::new(doc, plan)?;
    let mut ops: BTreeMap<(ObjectId, u32, u32), Vec<&PlannedCluster>> = BTreeMap::new();
    for page in &plan.pages {
        for c in &page.clusters {
            if c.font.is_none() {
                return Err(PdfError::at(ErrorCode::FontUnsupported, c.id.loc.stream));
            }
            let target = targets.stream(page.page_index, &c.id.path.0, c.id.loc.stream);
            ops.entry((target, c.id.loc.start, c.id.loc.end))
                .or_default()
                .push(c);
            report.clusters_removed += 1;
            report.invisible_clusters_removed += u32::from(c.invisible);
        }
    }

    let mut streams: HashMap<ObjectId, Vec<u8>> = HashMap::new();
    let mut rewrites = Vec::new();
    for ((target, start, end), group) in &ops {
        let stream = group[0].id.loc.stream;
        let data = match streams.entry(stream) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(decode(doc, stream)?),
        };
        let span = *start as usize..*end as usize;
        let font = group[0].font.ok_or(PdfError::new(ErrorCode::Internal))?;
        let width = code_width(doc, font)?;
        let new = rewrite(&data[span.clone()], stream, width, group)?;
        rewrites.push((*target, span, new));
    }
    report.operators_rewritten = rewrites.len() as u32;
    targets.apply(doc, plan, w)?;
    for (stream, span, new) in rewrites {
        w.replace_span(stream, span, new.as_bytes());
    }

    for (font, survivors) in &plan.font_survivors {
        let (subtype, name) = font_names(doc, *font)?;
        report.fonts_touched += 1;
        let program = fonts::touch_font(w, *font, survivors.iter())?;
        if program == Program::InventoryRetained || subtype == "Type3" {
            report.fonts_with_glyph_inventory.push((*font, name));
        }
    }
    Ok(report)
}

pub(super) fn decode(doc: &Document, id: ObjectId) -> Result<Vec<u8>, PdfError> {
    let r = doc.reader();
    let Some(s) = r.pdf().xref().get::<Stream<'_>>(at(id)) else {
        return Err(PdfError::at(ErrorCode::MalformedPdf, id));
    };
    let mut budget = Budget::new(r.limits());
    Ok(limits::decode_stream(&s, id, r.limits(), &mut budget)?.into_owned())
}

fn font_dict<'a>(doc: &'a Document, font: ObjectId) -> Result<Dict<'a>, PdfError> {
    doc.reader()
        .pdf()
        .xref()
        .get::<Dict<'a>>(at(font))
        .ok_or(PdfError::at(ErrorCode::MalformedPdf, font))
}

fn font_names(doc: &Document, font: ObjectId) -> Result<(String, String), PdfError> {
    let d = font_dict(doc, font)?;
    let name = |k: &str| {
        d.get::<Name<'_>>(k)
            .map(|n| n.as_str().to_owned())
            .unwrap_or_default()
    };
    let base = name("BaseFont");
    Ok((
        name("Subtype"),
        if base.is_empty() {
            format!("{} {} R", font.num, font.generation)
        } else {
            base
        },
    ))
}

/// Bytes per character code: 1 for simple fonts, 2 for Type0 with Identity-H/V.
fn code_width(doc: &Document, font: ObjectId) -> Result<usize, PdfError> {
    let d = font_dict(doc, font)?;
    if d.get::<Name<'_>>("Subtype")
        .is_some_and(|n| n.as_str() == "Type0")
    {
        return match d
            .get::<Name<'_>>("Encoding")
            .map(|n| n.as_str().to_owned())
            .as_deref()
        {
            Some("Identity-H" | "Identity-V") => Ok(2),
            _ => Err(PdfError::at(ErrorCode::FontUnsupported, font)),
        };
    }
    Ok(1)
}

/// `v` to `places` decimals, trailing zeros dropped, never in exponent form.
pub(crate) fn num(v: f64, places: usize) -> String {
    let s = format!("{v:.places$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0')
    } else {
        &s
    };
    let s = s.trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

/// The `TJ` array being built: kept codes collect in `buf`, removed codes and existing
/// adjustments in `pending`.
struct Tj {
    out: String,
    buf: Vec<u8>,
    pending: f64,
}

impl Tj {
    fn flush_string(&mut self) {
        if !self.buf.is_empty() {
            self.flush_shift();
            self.out.push_str(&string_token(&self.buf));
            self.buf.clear();
        }
    }

    fn flush_shift(&mut self) {
        if self.pending.abs() > 1e-9 {
            let _ = write!(self.out, " {} ", num(self.pending, 3));
        }
        self.pending = 0.0;
    }

    fn shift(&mut self, n: f64) {
        self.flush_string();
        self.pending += n;
    }
}

/// One show operator (`Tj`, `TJ`, `'`, `"`) with its operands, as `TJ` text without the
/// removed codes. `span` starts at the first operand.
fn rewrite(
    span: &[u8],
    stream: ObjectId,
    width: usize,
    removed: &[&PlannedCluster],
) -> Result<String, PdfError> {
    let bad = || PdfError::at(ErrorCode::MalformedPdf, stream);
    let mut it = UntypedIter::new(span);
    let ins = it.next().ok_or_else(bad)?;
    let operands: Vec<&Object<'_>> = ins.operands().collect();
    let (mut out, pieces): (String, Vec<Object<'_>>) = match (&**ins.operator, &operands[..]) {
        (b"Tj", [s]) => (String::new(), vec![(*s).clone()]),
        (b"'", [s]) => ("T*\n".into(), vec![(*s).clone()]),
        (b"\"", [Object::Number(aw), Object::Number(ac), s]) => (
            format!(
                "{} Tw\n{} Tc\nT*\n",
                num(aw.as_f64(), 5),
                num(ac.as_f64(), 5)
            ),
            vec![(*s).clone()],
        ),
        (b"TJ", [Object::Array(a)]) => (String::new(), a.iter::<Object<'_>>().collect()),
        _ => return Err(bad()),
    };
    let by_index: HashMap<u32, &PlannedCluster> =
        removed.iter().map(|c| (c.id.glyph, *c)).collect();
    let mut tj = Tj {
        out: String::from("["),
        buf: Vec::new(),
        pending: 0.0,
    };
    let (mut index, mut hit) = (0u32, 0usize);
    for o in &pieces {
        match o {
            Object::String(s) => {
                let bytes = s.as_bytes();
                if bytes.len() % width != 0 {
                    return Err(bad());
                }
                for code in bytes.chunks(width) {
                    match by_index.get(&index) {
                        Some(c) => {
                            let value = code.iter().fold(0u32, |a, b| (a << 8) | u32::from(*b));
                            if value != c.code {
                                // The index does not name the glyph the model reported.
                                return Err(PdfError::at(ErrorCode::Internal, stream));
                            }
                            tj.shift(-1000.0 * c.advance_em);
                            hit += 1;
                        }
                        None => tj.buf.extend_from_slice(code),
                    }
                    index += 1;
                }
            }
            // An existing adjustment stays, merged with the displacement beside it.
            Object::Number(n) => tj.shift(n.as_f64()),
            _ => return Err(bad()),
        }
    }
    tj.flush_string();
    tj.flush_shift();
    if hit != removed.len() {
        return Err(PdfError::at(ErrorCode::Internal, stream));
    }
    out.push_str(&tj.out);
    out.push_str("] TJ");
    Ok(out)
}
