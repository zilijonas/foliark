//! Browser-loadable copy of an embedded TrueType/OpenType program (spec §6.4 `editorFace`).
//!
//! Port of HDDN `_repair_font_for_browser` (pyodide.ts): PDF subset programs often ship no
//! usable Unicode cmap, no OS/2, no `name` and no `post`, and browsers refuse such a font or
//! draw the wrong glyph. The cmap is rebuilt from the glyph evidence (Unicode -> glyph id seen
//! on the document) unless the program's own Unicode cmap already agrees with all of it.
use skrifa::{FontRef, MetadataProvider};
use std::collections::BTreeMap;

type Tables = BTreeMap<[u8; 4], Vec<u8>>;

fn be16(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// OpenType table checksum: big-endian u32 words, the last one zero padded.
fn checksum(d: &[u8]) -> u32 {
    d.chunks(4).fold(0u32, |sum, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        sum.wrapping_add(u32::from_be_bytes(w))
    })
}

fn read_tables(buf: &[u8]) -> Option<Tables> {
    let n = usize::from(be16(buf, 4)?);
    let mut tables = Tables::new();
    for i in 0..n {
        let r = 12 + 16 * i;
        let tag: [u8; 4] = buf.get(r..r + 4)?.try_into().ok()?;
        let (off, len) = (be32(buf, r + 8)? as usize, be32(buf, r + 12)? as usize);
        tables.insert(tag, buf.get(off..off.checked_add(len)?)?.to_vec());
    }
    Some(tables)
}

fn write_tables(version: &[u8], mut tables: Tables) -> Option<Vec<u8>> {
    tables.get_mut(b"head")?.get_mut(8..12)?.fill(0);
    let n = tables.len();
    let pow = (1usize..).take_while(|p| p * 2 <= n).last().unwrap_or(1);
    let search_range = pow * 16;
    let mut out = version.to_vec();
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&(search_range as u16).to_be_bytes());
    out.extend_from_slice(&(pow.ilog2() as u16).to_be_bytes());
    out.extend_from_slice(&((n * 16 - search_range) as u16).to_be_bytes());
    let mut body = Vec::new();
    let mut head_at = 0;
    for (tag, data) in &tables {
        let at = 12 + 16 * n + body.len();
        if tag == b"head" {
            head_at = at;
        }
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&(at as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
        body.resize(body.len().next_multiple_of(4), 0);
    }
    out.extend_from_slice(&body);
    let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
    out[head_at + 8..head_at + 12].copy_from_slice(&adjustment.to_be_bytes());
    Some(out)
}

/// (3,1) format-4 cmap, one segment per code point plus the 0xFFFF terminator.
fn cmap_format4(pairs: &[(u16, u16)]) -> Vec<u8> {
    let segs = pairs.len() + 1;
    let pow = (1usize..).take_while(|p| p * 2 <= segs).last().unwrap_or(1);
    let u16s = |v: &mut Vec<u8>, it: &mut dyn Iterator<Item = u16>| {
        it.for_each(|x| v.extend_from_slice(&x.to_be_bytes()));
    };
    let mut sub = Vec::new();
    u16s(
        &mut sub,
        &mut [
            4,
            (16 + 8 * segs) as u16,
            0,
            (segs * 2) as u16,
            (pow * 2) as u16,
            pow.ilog2() as u16,
            ((segs - pow) * 2) as u16,
        ]
        .into_iter(),
    );
    let ends = || pairs.iter().map(|p| p.0).chain([0xFFFF]);
    u16s(&mut sub, &mut ends());
    u16s(&mut sub, &mut [0].into_iter()); // reservedPad
    u16s(&mut sub, &mut ends());
    u16s(
        &mut sub,
        &mut pairs.iter().map(|&(cp, g)| g.wrapping_sub(cp)).chain([1]),
    );
    u16s(&mut sub, &mut std::iter::repeat_n(0, segs));
    let mut out = vec![0, 0, 0, 1, 0, 3, 0, 1, 0, 0, 0, 12];
    out.extend(sub);
    out
}

/// OS/2 version 4 from hhea/head metrics, for a program that has none.
fn os2(tables: &Tables, bold: bool) -> Vec<u8> {
    let upem = tables
        .get(b"head")
        .and_then(|h| be16(h, 18))
        .unwrap_or(1000);
    let ascender = tables
        .get(b"hhea")
        .and_then(|h| be16(h, 4))
        .map_or(800, |v| v as i16);
    let descender = tables
        .get(b"hhea")
        .and_then(|h| be16(h, 6))
        .map_or(-200, |v| v as i16);
    let mut o = Vec::with_capacity(96);
    let mut w = |v: u16| o.extend_from_slice(&v.to_be_bytes());
    w(4); // version
    w(upem / 2); // xAvgCharWidth
    w(if bold { 700 } else { 400 }); // usWeightClass
    w(5); // usWidthClass
    w(0); // fsType
    for v in [650, 0, 650, 0, 0, 0, 0, 0, 0, 0] {
        w(v); // sub/superscript, strikeout
    }
    w(0); // sFamilyClass
    (0..5 + 8).for_each(|_| w(0)); // panose, ulUnicodeRange 1-4
    o.extend_from_slice(b"PfEd");
    let mut w = |v: u16| o.extend_from_slice(&v.to_be_bytes());
    w(if bold { 0x20 } else { 0x40 }); // fsSelection
    w(0x20); // usFirstCharIndex
    w(0xFFFF); // usLastCharIndex
    w(ascender as u16); // sTypoAscender
    w(descender as u16); // sTypoDescender
    w(0); // sTypoLineGap
    w(ascender.max(0) as u16); // usWinAscent
    w(descender.unsigned_abs()); // usWinDescent
    for v in [0, 1, 0, 0] {
        w(v); // ulCodePageRange 1-2
    }
    w(upem / 2); // sxHeight
    w(ascender.max(0) as u16); // sCapHeight
    w(0); // usDefaultChar
    w(0x20); // usBreakChar
    w(0); // usMaxContext
    debug_assert_eq!(o.len(), 96);
    o
}

/// `name` format 0 with family, subfamily, full and PostScript names (Windows, English).
fn name_table(family: &str, bold: bool) -> Vec<u8> {
    let ps: String = family.split_whitespace().collect();
    let style = if bold { "Bold" } else { "Regular" };
    let full = format!("{family} {style}");
    let records = [(1u16, family), (2, style), (4, &full), (6, &ps)];
    let mut head = vec![0, 0];
    for v in [records.len(), 6 + 12 * records.len()] {
        head.extend_from_slice(&(v as u16).to_be_bytes());
    }
    let mut strings = Vec::new();
    for (id, s) in records {
        let utf16: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
        for v in [3, 1, 0x409, id, utf16.len() as u16, strings.len() as u16] {
            head.extend_from_slice(&v.to_be_bytes());
        }
        strings.extend(utf16);
    }
    head.extend(strings);
    head
}

/// The program as a browser can load it: a (3,1) format-4 cmap that agrees with `pairs`
/// (Unicode, glyph id), OS/2, `name` and `post` added when missing; outlines and metrics are
/// untouched. `None` for anything that is not an sfnt (bare CFF, Type 1), an sfnt missing a
/// core table, or a cmap that cannot be expressed (code point or glyph beyond 16 bits).
pub fn repair(program: &[u8], family: &str, bold: bool, pairs: &[(u32, u32)]) -> Option<Vec<u8>> {
    if !matches!(program.get(..4)?, [0, 1, 0, 0] | b"true" | b"OTTO") {
        return None;
    }
    let mut tables = read_tables(program)?;
    let outlines = (tables.contains_key(b"glyf") && tables.contains_key(b"loca"))
        || tables.contains_key(b"CFF ");
    if !outlines
        || [b"head", b"hhea", b"hmtx", b"maxp"]
            .iter()
            .any(|t| !tables.contains_key(*t))
    {
        return None;
    }
    let mut pairs: Vec<(u16, u16)> = pairs
        .iter()
        .map(|&(c, g)| Some((u16::try_from(c).ok()?, u16::try_from(g).ok()?)))
        .collect::<Option<_>>()?;
    pairs.sort_unstable();
    pairs.dedup();
    // A cmap holds at most 8189 segments (16-bit table length).
    if pairs.is_empty() || pairs.len() > 8000 {
        return None;
    }
    let agrees = FontRef::new(program).ok().is_some_and(|f| {
        let map = f.charmap();
        tables.contains_key(b"cmap")
            && pairs
                .iter()
                .all(|&(c, g)| map.map(u32::from(c)).map(|x| x.to_u32()) == Some(u32::from(g)))
    });
    if !agrees {
        tables.insert(*b"cmap", cmap_format4(&pairs));
    }
    if !tables.contains_key(b"OS/2") {
        let t = os2(&tables, bold);
        tables.insert(*b"OS/2", t);
    }
    if !tables.contains_key(b"name") {
        tables.insert(*b"name", name_table(family, bold));
    }
    // post version 3: no glyph names, the outlines and advances stay as they are.
    tables.entry(*b"post").or_insert_with(|| {
        let mut p = vec![0, 3, 0, 0];
        p.resize(32, 0);
        p
    });
    write_tables(&program[..4], tables)
}
