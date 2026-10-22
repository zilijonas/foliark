//! Retained resources rebuilt from what content uses (spec §5.6, §8.2; NEW-45). The allowlist
//! writer already copies only listed keys, so image `/Metadata`, `/OPI`, `/Alternates`, font
//! `/CharSet`, `/CIDSet`, patterns, shadings and functions carry nothing extra. What it passes
//! through as opaque streams is audited here:
//!
//! - Fonts nothing removed text from (those touched by redaction are rebuilt by
//!   [`crate::redaction::text`]): ToUnicode rebuilt to the codes content draws, TrueType outlines
//!   of every other glyph emptied and the tables a PDF viewer does not need (`name`, kerning and
//!   layout tables, vendor tables) dropped, unused Type3 CharProcs emptied. A font whose program
//!   cannot be rebuilt (CFF, Type 1, a program shared by several fonts, text drawn where the model
//!   does not look) keeps it and is listed in `fontsWithGlyphInventory`.
//! - ICC profiles: header and declared size are checked, bytes past the declared size dropped; a
//!   stream that is no ICC profile of its `/N` refuses the export.
//! - JPEG images: every `APPn` (but Adobe), `COM` and everything after the end marker dropped;
//!   the entropy-coded data is untouched.
//!
//! Not audited, named in the receipt: JPX boxes, JBIG2 and CCITT payloads, the inside of an ICC
//! profile's declared size, Type3 CharProcs that draw text with another font.
use crate::Document;
use crate::interpret::{DiagnosticKind, GlyphItem, ItemKind};
use crate::reader::{self, object_id};
use crate::writer::fonts::{self, Program};
use crate::writer::{NewStream, Val, Writer};
use foliark_model::{ErrorCode, ObjectId, PdfError};
use hayro::hayro_syntax::object::{
    Array, Dict, MaybeRef, Name, Object, ObjectIdentifier as Oid, Stream,
};
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Fonts whose program is kept as it is: (font object, BaseFont).
    pub fonts_with_glyph_inventory: Vec<(ObjectId, String)>,
    pub fonts_rebuilt: u32,
    pub charprocs_emptied: u32,
    pub icc_trimmed: u32,
    pub jpegs_stripped: u32,
}

const FONT_SUBTYPES: [&str; 5] = ["Type0", "Type1", "MMType1", "TrueType", "Type3"];

#[derive(Default)]
struct Found {
    fonts: Vec<(ObjectId, Oid)>,
    icc: BTreeSet<Oid>,
    jpegs: Vec<Oid>,
}

fn visit(o: &Object<'_>, seen: &mut HashSet<Oid>, todo: &mut Vec<Oid>, f: &mut Found, depth: u8) {
    let mut child = |m: MaybeRef<Object<'_>>, f: &mut Found| match m {
        MaybeRef::Ref(r) => {
            let id = Oid::from(r);
            if seen.insert(id) {
                todo.push(id);
            }
        }
        MaybeRef::NotRef(o) if depth < 64 => visit(&o, seen, todo, f, depth + 1),
        MaybeRef::NotRef(_) => {}
    };
    match o {
        Object::Dict(d) => {
            for (_, v) in d.entries() {
                child(v, f);
            }
        }
        Object::Stream(s) => {
            for (_, v) in s.dict().entries() {
                child(v, f);
            }
        }
        Object::Array(a) => {
            let mut items = a.raw_iter();
            if let (Some(MaybeRef::NotRef(Object::Name(n))), Some(MaybeRef::Ref(r))) =
                (items.next(), items.next())
                && n.as_str() == "ICCBased"
            {
                f.icc.insert(Oid::from(r));
            }
            for v in a.raw_iter() {
                child(v, f);
            }
        }
        _ => {}
    }
}

/// Fonts, ICC streams and JPEG images reachable from the catalog.
fn find(doc: &reader::Document) -> Found {
    let xref = doc.pdf().xref();
    let root = xref.root_id();
    let (mut seen, mut todo, mut f) = (HashSet::from([root]), vec![root], Found::default());
    while let Some(id) = todo.pop() {
        let Some(o) = xref.get::<Object<'_>>(id) else {
            continue;
        };
        let subtype = |d: &Dict<'_>| d.get::<Name<'_>>("Subtype").map(|n| n.as_str().to_owned());
        match &o {
            Object::Dict(d) if subtype(d).is_some_and(|s| FONT_SUBTYPES.contains(&s.as_str())) => {
                f.fonts.push((object_id(id.into()), id));
            }
            Object::Stream(s)
                if subtype(s.dict()).as_deref() == Some("Image") && is_dct(s.dict()) =>
            {
                f.jpegs.push(id);
            }
            _ => {}
        }
        visit(&o, &mut seen, &mut todo, &mut f, 0);
    }
    f
}

/// A single `DCTDecode` filter, by name or as a one-item array.
fn is_dct(d: &Dict<'_>) -> bool {
    match d.get::<Object<'_>>("Filter") {
        Some(Object::Name(n)) => n.as_str() == "DCTDecode",
        Some(Object::Array(a)) => {
            let names: Vec<_> = a.iter::<Name<'_>>().collect();
            names.len() == 1 && names[0].as_str() == "DCTDecode"
        }
        _ => false,
    }
}

/// Glyphs content draws per font, and whether some paint is not seen by the model.
fn drawn(doc: &Document) -> Result<(HashMap<ObjectId, Vec<GlyphItem>>, bool), PdfError> {
    let (mut out, mut blind) = (HashMap::<ObjectId, Vec<GlyphItem>>::new(), false);
    let mut seen = HashSet::new();
    for i in 0..doc.page_count() {
        let m = doc.page_model(i)?;
        blind |= m
            .diagnostics
            .iter()
            .any(|d| matches!(d.kind, DiagnosticKind::PatternNotDescended));
        for it in m.items {
            if let ItemKind::Glyph(g) = it.kind
                && let Some(font) = g.font
                && seen.insert((font, g.char_code, g.gid))
            {
                out.entry(font).or_default().push(g);
            }
        }
    }
    Ok((out, blind))
}

fn program_of(font: &Dict<'_>) -> Option<ObjectId> {
    let owner = if font.get::<Name<'_>>("Subtype")?.as_str() == "Type0" {
        font.get::<Array<'_>>("DescendantFonts")?
            .iter::<Dict<'_>>()
            .next()?
    } else {
        font.clone()
    };
    let d = owner.get::<Dict<'_>>("FontDescriptor")?;
    ["FontFile2", "FontFile3", "FontFile"]
        .into_iter()
        .find_map(|k| match d.get_raw::<Object<'_>>(k)? {
            MaybeRef::Ref(r) => Some(object_id(r)),
            MaybeRef::NotRef(_) => None,
        })
}

/// Type3 CharProcs no drawn code points at become empty streams.
fn empty_unused_charprocs(font: &Dict<'_>, used: &[GlyphItem], w: &mut Writer<'_>) -> u32 {
    let (Some(procs), Some(enc)) = (
        font.get::<Dict<'_>>("CharProcs"),
        font.get::<Dict<'_>>("Encoding"),
    ) else {
        return 0;
    };
    let Some(diffs) = enc.get::<Array<'_>>("Differences") else {
        return 0;
    };
    let (mut code, mut names) = (0u32, HashMap::new());
    for o in diffs.iter::<Object<'_>>() {
        match o {
            Object::Number(n) => code = n.as_f64() as u32,
            Object::Name(n) => {
                names.insert(code, n.as_str().to_owned());
                code += 1;
            }
            _ => {}
        }
    }
    let live: HashSet<&String> = used
        .iter()
        .filter_map(|g| names.get(&g.char_code))
        .collect();
    let id = |k| match procs.get_raw::<Object<'_>>(k)? {
        MaybeRef::Ref(r) => Some(object_id(r)),
        MaybeRef::NotRef(_) => None,
    };
    let kept: HashSet<ObjectId> = procs
        .keys()
        .filter(|k| live.contains(&k.as_str().to_owned()))
        .filter_map(id)
        .collect();
    let mut n = 0;
    for k in procs.keys() {
        if let Some(p) = id(k).filter(|p| !kept.contains(p)) {
            w.replace_stream(
                p,
                NewStream {
                    dict: vec![],
                    data: vec![],
                },
            );
            n += 1;
        }
    }
    n
}

/// Length of the ICC profile at the start of `data`, if it is one for `n` components.
fn icc_len(data: &[u8], n: i32) -> Option<usize> {
    let size = u32::from_be_bytes(data.get(0..4)?.try_into().ok()?) as usize;
    let space_ok = matches!(
        (n, data.get(16..20)?),
        (1, b"GRAY") | (3, b"RGB " | b"Lab ") | (4, b"CMYK")
    );
    (space_ok && data.get(36..40)? == b"acsp" && (128..=data.len()).contains(&size)).then_some(size)
}

/// The JPEG without `APPn` (Adobe kept), `COM` and anything after the last end marker.
fn strip_jpeg(d: &[u8]) -> Option<Vec<u8>> {
    if !d.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let (mut out, mut i) = (vec![0xFF, 0xD8], 2);
    loop {
        if *d.get(i)? != 0xFF {
            return None;
        }
        while *d.get(i + 1)? == 0xFF {
            i += 1;
        }
        let m = d[i + 1];
        if m == 0xD9 {
            out.extend([0xFF, 0xD9]);
            return Some(out);
        }
        if m == 0x01 || (0xD0..=0xD7).contains(&m) {
            out.extend(&d[i..i + 2]);
            i += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([*d.get(i + 2)?, *d.get(i + 3)?]));
        if len < 2 {
            return None;
        }
        let seg = d.get(i..i + 2 + len)?;
        let adobe = m == 0xEE && len == 14 && seg[4..].starts_with(b"Adobe");
        if adobe || !((0xE0..=0xEF).contains(&m) || m == 0xFE) {
            out.extend(seg);
        }
        i += 2 + len;
        if m == 0xDA {
            let start = i;
            while i + 1 < d.len()
                && !(d[i] == 0xFF && d[i + 1] != 0 && !(0xD0..=0xD7).contains(&d[i + 1]))
            {
                i += 1;
            }
            out.extend(&d[start..i]);
        }
    }
}

/// Rebuild the retained resources of `doc` on `w` (the writer of `doc`). `touched` are the
/// fonts redaction already rebuilt: left alone.
pub fn rebuild(
    doc: &Document,
    touched: &BTreeSet<ObjectId>,
    w: &mut Writer<'_>,
) -> Result<Report, PdfError> {
    let reader = doc.reader();
    let xref = reader.pdf().xref();
    let found = find(reader);
    let mut report = Report::default();

    let (glyphs, blind) = drawn(doc)?;
    let mut sharing: HashMap<ObjectId, u32> = HashMap::new();
    for (_, oid) in &found.fonts {
        if let Some(p) = xref.get::<Dict<'_>>(*oid).as_ref().and_then(program_of) {
            *sharing.entry(p).or_default() += 1;
        }
    }
    for (font, oid) in &found.fonts {
        let Some(d) = xref.get::<Dict<'_>>(*oid) else {
            continue;
        };
        if touched.contains(font) {
            continue;
        }
        let name = d.get::<Name<'_>>("BaseFont").map_or_else(
            || format!("{} {} R", font.num, font.generation),
            |n| n.as_str().to_owned(),
        );
        let program = program_of(&d);
        let inventory = |report: &mut Report| {
            report
                .fonts_with_glyph_inventory
                .push((*font, name.clone()));
        };
        // A shared program needs the union of its fonts' glyphs, and text the model does not
        // see (pattern cells) could draw any glyph: keep the program, say so.
        if program.is_some_and(|p| blind || sharing[&p] > 1) {
            inventory(&mut report);
            continue;
        }
        let used = glyphs.get(font).map_or(&[][..], Vec::as_slice);
        if d.get::<Name<'_>>("Subtype")
            .is_some_and(|s| s.as_str() == "Type3")
            && !blind
        {
            report.charprocs_emptied += empty_unused_charprocs(&d, used, w);
        }
        // Nothing hidden to rebuild: no program and no ToUnicode (a Base-14 font).
        if program.is_none() && !d.contains_key("ToUnicode") {
            continue;
        }
        match fonts::touch_font(w, *font, used) {
            Ok(Program::Emptied { .. }) => report.fonts_rebuilt += 1,
            Ok(Program::InventoryRetained) => inventory(&mut report),
            Ok(Program::None) => report.fonts_rebuilt += 1,
            Err(e) if e.code == ErrorCode::FontUnsupported => inventory(&mut report),
            Err(e) => return Err(e),
        }
    }

    for id in &found.icc {
        let Some(Object::Stream(s)) = xref.get::<Object<'_>>(*id) else {
            continue;
        };
        let oid = object_id((*id).into());
        let data = decode(&s, oid, reader)?;
        let n = s.dict().get::<i32>("N").unwrap_or(0);
        match icc_len(&data, n) {
            None => return Err(PdfError::at(ErrorCode::MalformedPdf, oid)),
            Some(size) if size < data.len() => {
                w.replace_stream(
                    oid,
                    NewStream {
                        dict: vec![("N".into(), Val::Int(n))],
                        data: data[..size].to_vec(),
                    },
                );
                report.icc_trimmed += 1;
            }
            Some(_) => {}
        }
    }

    for id in &found.jpegs {
        let Some(Object::Stream(s)) = xref.get::<Object<'_>>(*id) else {
            continue;
        };
        let oid = object_id((*id).into());
        let raw = s
            .raw_data_strict()
            .map_err(|_| PdfError::at(ErrorCode::DecodeFailed, oid))?;
        let clean = strip_jpeg(&raw).ok_or(PdfError::at(ErrorCode::DecodeFailed, oid))?;
        if clean.len() < raw.len() {
            w.replace_stream_data(oid, clean);
            report.jpegs_stripped += 1;
        }
    }
    report
        .fonts_with_glyph_inventory
        .sort_by(|a, b| a.1.cmp(&b.1));
    Ok(report)
}

fn decode(s: &Stream<'_>, id: ObjectId, doc: &reader::Document) -> Result<Vec<u8>, PdfError> {
    let mut budget = crate::limits::Budget::new(doc.limits());
    Ok(crate::limits::decode_stream(s, id, doc.limits(), &mut budget)?.into_owned())
}
