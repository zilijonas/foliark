//! Font side of the writer (spec §5.6, §6.3). Fonts touched by removal get a ToUnicode that
//! maps only surviving codes and, for TrueType programs, emptied outlines of unused glyphs
//! (ids stay stable). Registered fonts are written as Type0 / Identity-H subsets.
//!
//! `/CharSet` and `/CIDSet` need no code here: the font-descriptor allowlist in `graph.rs`
//! never copies them.
use super::{NewStream, Val, Writer, err, err_obj};
use crate::interpret::GlyphItem;
use crate::limits;
use crate::reader::object_id;
use foliark_model::{ErrorCode, ObjectId, PdfError};
use foliark_text::{LayoutPlan, RegisteredFace};
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, ObjectIdentifier, Stream};
use skrifa::{FontRef, MetadataProvider, instance::LocationRef, instance::Size};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

// ---- ToUnicode ------------------------------------------------------------------------------

fn utf16_hex(text: &str) -> String {
    text.encode_utf16().fold(String::new(), |mut s, u| {
        let _ = write!(s, "{u:04X}");
        s
    })
}

/// A ToUnicode CMap (plain text) that maps exactly the given `(code, text)` pairs; codes are
/// `code_bytes` wide (1 for simple fonts, 2 for Identity-H). Empty text is skipped, the first
/// text for a code wins, a code wider than `code_bytes` is `INVALID_ARGUMENT`.
pub fn rebuild_to_unicode<S: AsRef<str>>(
    code_bytes: u8,
    mappings: impl IntoIterator<Item = (u32, S)>,
) -> Result<Vec<u8>, PdfError> {
    if !(1..=4).contains(&code_bytes) {
        return Err(err(ErrorCode::InvalidArgument, "code width"));
    }
    let width = usize::from(code_bytes) * 2;
    let mut map: BTreeMap<u32, String> = BTreeMap::new();
    for (code, text) in mappings {
        if u64::from(code) >> (8 * u32::from(code_bytes)) != 0 {
            return Err(err(ErrorCode::InvalidArgument, "code wider than codespace"));
        }
        if !text.as_ref().is_empty() {
            map.entry(code).or_insert_with(|| text.as_ref().to_owned());
        }
    }
    let mut s = format!(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<{}> <{}>\nendcodespacerange\n",
        "0".repeat(width),
        "F".repeat(width)
    );
    let entries: Vec<_> = map.into_iter().collect();
    for chunk in entries.chunks(100) {
        let _ = writeln!(s, "{} beginbfchar", chunk.len());
        for (code, text) in chunk {
            let _ = writeln!(s, "<{code:0width$X}> <{}>", utf16_hex(text));
        }
        s.push_str("endbfchar\n");
    }
    s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    Ok(s.into_bytes())
}

// ---- TrueType glyph emptying ---------------------------------------------------------------

const MALFORMED: &str = "malformed TrueType program";

fn be16(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// Sum of big-endian u32 words, the last one zero padded (OpenType table checksum).
fn checksum(d: &[u8]) -> u32 {
    d.chunks(4).fold(0u32, |sum, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        sum.wrapping_add(u32::from_be_bytes(w))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramKind {
    /// sfnt with `glyf` outlines: glyphs can be emptied.
    TrueType,
    /// Anything else (CFF OpenType, Type 1 ...): kept as is.
    Other,
}

pub fn program_kind(program: &[u8]) -> ProgramKind {
    let truetype = matches!(program.get(..4), Some([0, 1, 0, 0] | b"true"));
    let has_glyf = truetype
        && be16(program, 4).is_some_and(|n| {
            (0..usize::from(n)).any(|i| program.get(12 + 16 * i..16 + 16 * i) == Some(b"glyf"))
        });
    if has_glyf {
        ProgramKind::TrueType
    } else {
        ProgramKind::Other
    }
}

/// Glyphs referenced by a composite glyph.
fn components(glyph: &[u8]) -> Option<Vec<u16>> {
    if glyph.len() < 10 || be16(glyph, 0)? < 0x8000 {
        return Some(Vec::new());
    }
    let (mut out, mut at) = (Vec::new(), 10);
    loop {
        let flags = be16(glyph, at)?;
        out.push(be16(glyph, at + 2)?);
        at += 4 + if flags & 1 != 0 { 4 } else { 2 };
        at += match (flags & 0x8 != 0, flags & 0x40 != 0, flags & 0x80 != 0) {
            (_, _, true) => 8,
            (_, true, _) => 4,
            (true, _, _) => 2,
            _ => 0,
        };
        if flags & 0x20 == 0 {
            return Some(out);
        }
    }
}

/// The program with the outline of every glyph outside `used` (plus `.notdef` and the
/// components of kept composites) made zero length in `glyf`; `loca` is rewritten, every other
/// table and every glyph id stays. `head.checkSumAdjustment` and table checksums are redone.
pub fn empty_unused_truetype_glyphs(
    program: &[u8],
    used: &BTreeSet<u32>,
) -> Result<Vec<u8>, PdfError> {
    let bad = || err(ErrorCode::FontUnsupported, MALFORMED);
    if program_kind(program) != ProgramKind::TrueType {
        return Err(err(ErrorCode::FontUnsupported, "not a TrueType program"));
    }
    let n_tables = usize::from(be16(program, 4).ok_or_else(bad)?);
    // (tag, offset, length) in directory order
    let mut tables: Vec<([u8; 4], usize, usize)> = Vec::new();
    for i in 0..n_tables {
        let r = 12 + 16 * i;
        let tag: [u8; 4] = program
            .get(r..r + 4)
            .ok_or_else(bad)?
            .try_into()
            .map_err(|_| bad())?;
        let (off, len) = (
            be32(program, r + 8).ok_or_else(bad)? as usize,
            be32(program, r + 12).ok_or_else(bad)? as usize,
        );
        if off.checked_add(len).is_none_or(|e| e > program.len()) {
            return Err(bad());
        }
        tables.push((tag, off, len));
    }
    let table = |tag: &[u8; 4]| {
        tables
            .iter()
            .find(|t| &t.0 == tag)
            .map(|&(_, o, l)| &program[o..o + l])
            .ok_or_else(bad)
    };
    let (head, maxp, loca, glyf) = (
        table(b"head")?,
        table(b"maxp")?,
        table(b"loca")?,
        table(b"glyf")?,
    );
    let long = be16(head, 50).ok_or_else(bad)? != 0;
    let n_glyphs = usize::from(be16(maxp, 4).ok_or_else(bad)?);
    let step = if long { 4 } else { 2 };
    if loca.len() < (n_glyphs + 1) * step {
        return Err(bad());
    }
    let offset = |g: usize| -> Option<usize> {
        let v = if long {
            be32(loca, g * 4)? as usize
        } else {
            usize::from(be16(loca, g * 2)?) * 2
        };
        Some(v)
    };
    let span = |g: usize| -> Result<&[u8], PdfError> {
        let (a, b) = (offset(g).ok_or_else(bad)?, offset(g + 1).ok_or_else(bad)?);
        glyf.get(a..b).ok_or_else(bad)
    };
    // Keep set: .notdef, the used glyphs and the closure over composite components.
    let mut keep: BTreeSet<usize> = BTreeSet::from([0]);
    let mut todo: Vec<usize> = used
        .iter()
        .map(|&g| g as usize)
        .filter(|&g| g < n_glyphs)
        .collect();
    while let Some(g) = todo.pop() {
        if !keep.insert(g) && g != 0 {
            continue;
        }
        for c in components(span(g)?).ok_or_else(bad)? {
            let c = usize::from(c);
            if c >= n_glyphs {
                return Err(bad());
            }
            if !keep.contains(&c) {
                todo.push(c);
            }
        }
    }
    let (mut new_glyf, mut new_loca) = (Vec::new(), Vec::with_capacity((n_glyphs + 1) * step));
    let align = step;
    for g in 0..=n_glyphs {
        let at = new_glyf.len();
        if long {
            new_loca.extend_from_slice(&(at as u32).to_be_bytes());
        } else {
            let half = u16::try_from(at / 2).map_err(|_| bad())?;
            new_loca.extend_from_slice(&half.to_be_bytes());
        }
        if g < n_glyphs && keep.contains(&g) {
            new_glyf.extend_from_slice(span(g)?);
            while new_glyf.len() % align != 0 {
                new_glyf.push(0);
            }
        }
    }
    // Reassemble: the same directory order, 4-byte aligned tables. Only the tables a PDF
    // viewer reads stay: `name` (copyright, designer), layout, kerning, `DSIG` and vendor tables
    // go, `post` keeps the names of the glyphs that stay.
    const NEEDED: [&[u8; 4]; 13] = [
        b"head", b"hhea", b"maxp", b"loca", b"glyf", b"hmtx", b"cmap", b"post", b"cvt ", b"fpgm",
        b"prep", b"OS/2", b"vmtx",
    ];
    tables.retain(|t| NEEDED.contains(&&t.0) || &t.0 == b"vhea");
    let n_tables = tables.len();
    let sel = n_tables.max(1).ilog2() as u16;
    let range = 16 * (1u16 << sel);
    let mut out = program[..4].to_vec();
    for v in [n_tables as u16, range, sel, n_tables as u16 * 16 - range] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut data = Vec::new();
    let base = 12 + 16 * n_tables;
    let mut records = Vec::new();
    for &(tag, off, len) in &tables {
        let mut body = match &tag {
            b"glyf" => new_glyf.clone(),
            b"loca" => new_loca.clone(),
            b"post" => post_names(&program[off..off + len], &keep, n_glyphs),
            _ => program[off..off + len].to_vec(),
        };
        if &tag == b"head" {
            body.get_mut(8..12).ok_or_else(bad)?.fill(0);
        }
        let at = base + data.len();
        records.push((tag, checksum(&body), at, body.len()));
        data.extend_from_slice(&body);
        while data.len() % 4 != 0 {
            data.push(0);
        }
    }
    for (tag, sum, at, len) in &records {
        out.extend_from_slice(tag);
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&(*at as u32).to_be_bytes());
        out.extend_from_slice(&(*len as u32).to_be_bytes());
    }
    out.extend_from_slice(&data);
    let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
    let head_at = records
        .iter()
        .find(|r| &r.0 == b"head")
        .map(|r| r.2)
        .ok_or_else(bad)?;
    out[head_at + 8..head_at + 12].copy_from_slice(&adjustment.to_be_bytes());
    Ok(out)
}

// ---- Fonts touched by removal ---------------------------------------------------------------

/// `post` with the glyph names of glyphs outside `keep` gone: format 2.0 gets `.notdef` for
/// them and only the strings the kept glyphs name; other formats carry no per-glyph names.
fn post_names(post: &[u8], keep: &BTreeSet<usize>, n_glyphs: usize) -> Vec<u8> {
    if be32(post, 0) != Some(0x0002_0000) || usize::from(be16(post, 32).unwrap_or(0)) != n_glyphs {
        return post.to_vec();
    }
    let mut strings = Vec::new();
    let mut at = 34 + 2 * n_glyphs;
    while let Some(&len) = post.get(at) {
        let end = at + 1 + usize::from(len);
        match post.get(at + 1..end) {
            Some(s) => strings.push(s),
            None => break,
        }
        at = end;
    }
    let mut out = post[..34].to_vec();
    let (mut names, mut used): (Vec<u8>, Vec<usize>) = (Vec::new(), Vec::new());
    for g in 0..n_glyphs {
        let idx = usize::from(be16(post, 34 + 2 * g).unwrap_or(0));
        let new = match idx {
            _ if !keep.contains(&g) => 0,
            0..258 => idx,
            _ => match strings.get(idx - 258) {
                Some(s) => {
                    let n = used.iter().position(|&u| u == idx).unwrap_or_else(|| {
                        used.push(idx);
                        names.push(s.len() as u8);
                        names.extend_from_slice(s);
                        used.len() - 1
                    });
                    258 + n
                }
                None => 0,
            },
        };
        out.extend_from_slice(&(new as u16).to_be_bytes());
    }
    out.extend(names);
    out
}

/// What happened to the font program of a touched font (spec §5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    /// TrueType: outlines of glyphs not referenced by surviving content emptied in place.
    Emptied { kept: usize, emptied: usize },
    /// CFF / Type 1 / other: kept untouched, the caller reports `fontsWithGlyphInventory`.
    InventoryRetained,
    /// No embedded program (base-14 style or Type3).
    None,
}

fn dict<'a>(w: &Writer<'a>, id: ObjectId) -> Option<Object<'a>> {
    w.doc.pdf().xref().get::<Object<'a>>(ObjectIdentifier::new(
        id.num as i32,
        i32::from(id.generation),
    ))
}

/// Source font `font` after content removal. `survivors` are the glyphs that remain drawn
/// (from the semantic model; those of other fonts are ignored): the font's ToUnicode becomes
/// a map of their codes and a TrueType program loses every other outline. If several fonts
/// share one program, pass the survivors of all of them to each call. Type0 fonts other than
/// Identity-H/V and `Uni*-UCS2-*` are refused (`FONT_UNSUPPORTED`), their code width is not known.
pub fn touch_font<'g>(
    w: &mut Writer<'_>,
    font: ObjectId,
    survivors: impl IntoIterator<Item = &'g GlyphItem>,
) -> Result<Program, PdfError> {
    let refuse = |m: &str| err_obj(ErrorCode::FontUnsupported, m, font);
    let Some(Object::Dict(fd)) = dict(w, font) else {
        return Err(err_obj(
            ErrorCode::MalformedPdf,
            "font is not a dictionary",
            font,
        ));
    };
    let survivors: Vec<&GlyphItem> = survivors
        .into_iter()
        .filter(|g| g.font == Some(font))
        .collect();
    let subtype = fd
        .get::<Name<'_>>("Subtype")
        .ok_or_else(|| refuse("no font subtype"))?;
    let (code_bytes, program_owner): (u8, Dict<'_>) = match subtype.as_str() {
        "Type1" | "MMType1" | "TrueType" => (1, fd.clone()),
        "Type3" => (1, fd.clone()),
        "Type0" => {
            let fixed = fd
                .get::<Name<'_>>("Encoding")
                .and_then(|n| crate::cmaps::fixed_code_width(n.as_str()));
            if fixed.is_none() {
                return Err(refuse("Type0 font without a fixed two-byte encoding"));
            }
            let desc = fd
                .get::<hayro::hayro_syntax::object::Array<'_>>("DescendantFonts")
                .and_then(|a| a.iter::<Dict<'_>>().next())
                .ok_or_else(|| refuse("Type0 font without a descendant"))?;
            (2, desc)
        }
        _ => return Err(refuse("unknown font subtype")),
    };
    let cmap = rebuild_to_unicode(
        code_bytes,
        survivors
            .iter()
            .filter_map(|g| g.unicode.as_deref().map(|u| (g.char_code, u))),
    )?;
    w.set_to_unicode(font, cmap);
    let Some(desc) = program_owner.get::<Dict<'_>>("FontDescriptor") else {
        return Ok(Program::None);
    };
    let Some((key, MaybeRef::Ref(r))) = ["FontFile2", "FontFile3", "FontFile"]
        .into_iter()
        .find_map(|k| Some((k, desc.get_raw::<Object<'_>>(k)?)))
    else {
        return Ok(Program::None);
    };
    let program_id = object_id(r);
    let Some(Object::Stream(s)) = dict(w, program_id) else {
        return Err(err_obj(
            ErrorCode::MalformedPdf,
            "font program is not a stream",
            program_id,
        ));
    };
    let mut budget = limits::Budget::new(&w.limits);
    let bytes = limits::decode_stream(&s, program_id, &w.limits, &mut budget)?;
    if key == "FontFile" || program_kind(&bytes) != ProgramKind::TrueType {
        return Ok(Program::InventoryRetained);
    }
    let used: BTreeSet<u32> = survivors.iter().filter_map(|g| g.gid).collect();
    let emptied = empty_unused_truetype_glyphs(&bytes, &used)?;
    let mut dict = vec![("Length1".to_owned(), Val::Int(emptied.len() as i32))];
    if let Some(sub) = stream_subtype(&s) {
        dict.push(("Subtype".to_owned(), Val::Name(sub)));
    }
    let total = glyph_count(&bytes).unwrap_or(0);
    let kept = used
        .iter()
        .filter(|&&g| (g as usize) < total)
        .count()
        .max(1);
    w.replace_stream(
        program_id,
        NewStream {
            dict,
            data: emptied,
        },
    );
    Ok(Program::Emptied {
        kept,
        emptied: total.saturating_sub(kept),
    })
}

fn stream_subtype(s: &Stream<'_>) -> Option<Vec<u8>> {
    s.dict()
        .get::<Name<'_>>("Subtype")
        .map(|n| n.as_str().as_bytes().to_vec())
}

fn glyph_count(program: &[u8]) -> Option<usize> {
    let n = usize::from(be16(program, 4)?);
    (0..n).find_map(|i| {
        let r = 12 + 16 * i;
        (program.get(r..r + 4)? == b"maxp")
            .then(|| be16(program, be32(program, r + 8)? as usize + 4).map(usize::from))
            .flatten()
    })
}

// ---- Registered fonts -----------------------------------------------------------------------

/// Glyphs of one registered font that content uses, with the text each stands for.
#[derive(Debug, Default, Clone)]
pub struct UsedGlyphs {
    glyphs: BTreeMap<u16, String>,
    /// Glyphs an earlier embedding of the font already holds, in CID order (CID 1, 2, ...).
    prior: Vec<u16>,
}

impl UsedGlyphs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start from the glyphs an earlier embedding of the font holds ([`registered_state`]):
    /// they keep their CIDs, so content that already uses the old font still reads right.
    pub fn from_prior(prior: Vec<(u16, String)>) -> Self {
        let mut used = Self::default();
        for (gid, text) in prior {
            used.prior.push(gid);
            used.glyphs.insert(gid, text);
        }
        used
    }

    /// Add one glyph and its text; the first text for a glyph wins.
    pub fn add(&mut self, gid: u16, text: &str) {
        self.glyphs.entry(gid).or_insert_with(|| text.to_owned());
    }

    /// Every glyph the runs of `plan` set in registered font `font_id`; `text` is the
    /// normalized paragraph the plan was made from (cluster ranges are UTF-16 offsets into it).
    /// A glyph that shares its cluster with the one before it (a mark kept apart from its base)
    /// gets no text of its own.
    pub fn add_plan(&mut self, plan: &LayoutPlan, font_id: &str, text: &str) {
        let mut at: Vec<(u32, usize)> = Vec::new(); // (utf16 offset, byte offset)
        let mut u = 0u32;
        for (b, c) in text.char_indices() {
            at.push((u, b));
            u += c.len_utf16() as u32;
        }
        at.push((u, text.len()));
        let byte = |off: u32| at.iter().find(|(o, _)| *o == off).map(|&(_, b)| b);
        for run in plan.lines.iter().flat_map(|l| &l.runs) {
            if run.font != font_id {
                continue;
            }
            let mut previous: Option<u32> = None;
            for g in &run.glyphs {
                let shown = match (byte(g.cluster.start), byte(g.cluster.end)) {
                    (Some(a), Some(b)) if previous != Some(g.cluster.start) => &text[a..b],
                    _ => "",
                };
                previous = Some(g.cluster.start);
                if let Ok(gid) = u16::try_from(g.gid) {
                    self.add(gid, shown);
                }
            }
        }
    }
}

/// A registered font as written: the Type0 font object and the glyph id -> CID table of the
/// subset (CID = new glyph id, CIDToGIDMap Identity).
#[derive(Debug, Clone)]
pub struct EmbeddedFont {
    pub font: Val,
    pub cid: BTreeMap<u16, u16>,
    /// Advance per CID in thousandths of an em, as written to /W.
    pub widths: BTreeMap<u16, f64>,
}

/// FNV-1a over the registry font id: the `ABCDEF+` subset tag. It does not depend on the glyph
/// set, so one font keeps its tag when later edits add glyphs, and the tag finds it again.
fn subset_tag(font_id: &str) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in font_id.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    (0..6)
        .map(|i| char::from(b'A' + ((h >> (i * 8)) % 26) as u8))
        .collect()
}

/// The page resource name of registered font `font_id` (one name per font, document-wide).
pub fn registered_resource(font_id: &str) -> String {
    format!("FkR{}", subset_tag(font_id))
}

/// Key of the Type0 font dictionary that lists the glyphs in CID order, each as gid and text
/// (what [`UsedGlyphs::from_prior`] needs). Working copies keep it; export drops it.
const GLYPHS_KEY: &str = "FoliarkGlyphs";

/// What [`embed_registered_font`] recorded in `font`, a font dictionary of a working copy.
pub fn registered_state(font: &Dict<'_>) -> Option<Vec<(u16, String)>> {
    let items: Vec<Object<'_>> = font.get::<Array<'_>>(GLYPHS_KEY)?.iter().collect();
    items
        .chunks(2)
        .map(|pair| match pair {
            [Object::Number(g), Object::String(t)] => Some((
                g.as_f64() as u16,
                String::from_utf8(t.as_bytes().to_vec()).ok()?,
            )),
            _ => None,
        })
        .collect()
}

fn array(items: impl IntoIterator<Item = Val>) -> Val {
    Val::Arr(items.into_iter().collect())
}

fn name(n: &str) -> Val {
    Val::Name(n.as_bytes().to_vec())
}

fn entry(k: &str, v: Val) -> (Vec<u8>, Val) {
    (k.as_bytes().to_vec(), v)
}

/// Write `face` as a Type0 / Identity-H font (CIDFontType2, or CIDFontType0 for CFF) holding
/// only the glyphs in `used`, with /W from the font's advances and a ToUnicode from the
/// recorded text. One font object per (font id, document): a second call for the same face
/// is `INVALID_ARGUMENT`, so collect all text for the font first. Use `Writer::add_page_font`
/// with the returned `font` to make it drawable.
pub fn embed_registered_font(
    w: &mut Writer<'_>,
    face: &RegisteredFace,
    used: &UsedGlyphs,
) -> Result<EmbeddedFont, PdfError> {
    let id = face.info().id.clone();
    if w.embedded.contains(&id) {
        return Err(err(ErrorCode::InvalidArgument, "font already embedded"));
    }
    let unsupported = |m: &str| err(ErrorCode::FontUnsupported, m);
    // Earlier glyphs first, in their old order: their CIDs do not move.
    let mut remap = subsetter::GlyphRemapper::new();
    for &g in used.prior.iter().chain(used.glyphs.keys()) {
        remap.remap(g);
    }
    let subset = subsetter::subset(face.bytes(), 0, &remap)
        .map_err(|_| unsupported("font cannot be subset"))?;
    let cid: BTreeMap<u16, u16> = used
        .glyphs
        .keys()
        .filter_map(|&g| remap.get(g).map(|n| (g, n)))
        .collect();
    let font = FontRef::new(face.bytes()).map_err(|_| unsupported("font cannot be parsed"))?;
    let m = font.metrics(Size::unscaled(), LocationRef::default());
    let upem = f64::from(m.units_per_em.max(1));
    let k = 1000.0 / upem;
    let glyph_metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    // /W: one entry per CID, in CID order (0 is .notdef and keeps the default width).
    let mut widths: Vec<(u16, f32)> = Vec::new();
    for (&old, &new) in &cid {
        let adv = glyph_metrics
            .advance_width(skrifa::GlyphId::new(u32::from(old)))
            .unwrap_or(0.0);
        widths.push((new, (f64::from(adv) * k) as f32));
    }
    widths.sort_by_key(|&(c, _)| c);
    let width_map: BTreeMap<u16, f64> = widths.iter().map(|&(c, x)| (c, f64::from(x))).collect();
    let mut w_array: Vec<Val> = Vec::new();
    let mut run: Vec<(u16, f32)> = Vec::new();
    let flush = |run: &mut Vec<(u16, f32)>, out: &mut Vec<Val>| {
        if let Some(&(first, _)) = run.first() {
            out.push(Val::Int(i32::from(first)));
            out.push(array(run.iter().map(|&(_, x)| Val::Real(x))));
        }
        run.clear();
    };
    for (c, x) in widths {
        if run.last().is_some_and(|&(p, _)| p + 1 != c) {
            flush(&mut run, &mut w_array);
        }
        run.push((c, x));
    }
    flush(&mut run, &mut w_array);

    let cff = subset.get(..4) == Some(b"OTTO");
    let ps = format!("{}+{}", subset_tag(&id), face.info().postscript_name);
    let bold = face.info().weight >= 700;
    let italic = face.info().style == foliark_model::FontStyle::Italic;
    let flags = 32 | if italic { 64 } else { 0 } | if bold { 1 << 18 } else { 0 };
    let bbox = m
        .bounds
        .map_or([0.0; 4], |b| [b.x_min, b.y_min, b.x_max, b.y_max]);
    let mut file = vec![];
    if cff {
        file.push(entry("Subtype", name("OpenType")));
    } else {
        file.push(entry("Length1", Val::Int(subset.len() as i32)));
    }
    let file = w.add_object(Val::Dict(file), Some(subset));
    let real = |v: f32| Val::Real((f64::from(v) * k) as f32);
    let descriptor = w.add_object(
        Val::Dict(vec![
            entry("Type", name("FontDescriptor")),
            entry("FontName", name(&ps)),
            entry("Flags", Val::Int(flags)),
            entry("FontBBox", array(bbox.map(real))),
            entry("ItalicAngle", Val::Real(m.italic_angle)),
            entry("Ascent", real(m.ascent)),
            entry("Descent", real(m.descent)),
            entry("CapHeight", real(m.cap_height.unwrap_or(m.ascent * 0.7))),
            entry("StemV", Val::Int(if bold { 140 } else { 80 })),
            entry(if cff { "FontFile3" } else { "FontFile2" }, file),
        ]),
        None,
    );
    let mut cid_font = vec![
        entry("Type", name("Font")),
        entry(
            "Subtype",
            name(if cff { "CIDFontType0" } else { "CIDFontType2" }),
        ),
        entry("BaseFont", name(&ps)),
        entry(
            "CIDSystemInfo",
            Val::Dict(vec![
                entry("Registry", Val::Str(b"Adobe".to_vec())),
                entry("Ordering", Val::Str(b"Identity".to_vec())),
                entry("Supplement", Val::Int(0)),
            ]),
        ),
        entry("FontDescriptor", descriptor),
        entry("DW", Val::Int(0)),
        entry("W", Val::Arr(w_array)),
    ];
    if !cff {
        cid_font.push(entry("CIDToGIDMap", name("Identity")));
    }
    let cid_font = w.add_object(Val::Dict(cid_font), None);
    let cmap = rebuild_to_unicode(
        2,
        cid.iter()
            .filter_map(|(old, new)| Some((u32::from(*new), used.glyphs.get(old)?.as_str()))),
    )?;
    let to_unicode = w.add_object(Val::Dict(Vec::new()), Some(cmap));
    let mut in_cid_order: Vec<_> = cid.iter().map(|(old, new)| (*new, *old)).collect();
    in_cid_order.sort_unstable();
    let record = in_cid_order.into_iter().flat_map(|(_, old)| {
        let text = used
            .glyphs
            .get(&old)
            .map_or(Vec::new(), |t| t.as_bytes().to_vec());
        [Val::Int(i32::from(old)), Val::Str(text)]
    });
    let type0 = w.add_object(
        Val::Dict(vec![
            entry("Type", name("Font")),
            entry("Subtype", name("Type0")),
            entry("BaseFont", name(&ps)),
            entry("Encoding", name("Identity-H")),
            entry("DescendantFonts", array([cid_font])),
            entry("ToUnicode", to_unicode),
            entry(GLYPHS_KEY, array(record)),
        ]),
        None,
    );
    w.embedded.insert(id);
    Ok(EmbeddedFont {
        font: type0,
        cid,
        widths: width_map,
    })
}
