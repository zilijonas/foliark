//! Rung 2 of the font ladder (spec §6.4): a PDF font reused as-is for edited text.
//!
//! What the source font can draw is recovered from glyph evidence, as HDDN does from
//! `get_texttrace`: every glyph the interpreter reports for the font gives one
//! `(code, Unicode, glyph id)`. A character is *covered* when
//! - exactly one code shows it (the lowest such code), that code shows no other character,
//!   and the Unicode is a single BMP character (the editor cmap is format 4);
//! - the glyph exists: a non-zero id inside the embedded sfnt program;
//! - the advance is listed (`/Widths` of the code, `/W` of the CID or a non-zero `/DW`): no
//!   advance is ever guessed.
//!
//! Characters never drawn with the font are not covered even when the program has them
//! (a full, unsubsetted font is not probed); the ladder then falls to a registered font.
//! Reusable fonts are simple Type1/TrueType and Type0 with the Identity-H CMap; Type3, other
//! CMaps and Identity-V (the editable layout is horizontal) are not (`Ok(None)`).
//! A font without an embedded program (Base-14 or system) is reused too, since the viewer draws
//! it the same way it draws the original text; [`SourceApprox::embedded`] records that.
use super::editor_face;
use crate::Document;
use crate::interpret::{FontInfo, FontKind, GlyphItem};
use crate::limits;
use crate::reader::object_id;
use foliark_model::{EditorFace, ErrorCode, ObjectId, PdfError, RunSource};
use foliark_text::{FaceMetrics, Fnv, FontFace, Glyph};
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, ObjectIdentifier};
use skrifa::{FontRef, MetadataProvider, instance::LocationRef, instance::Size};
use std::collections::BTreeMap;

/// What makes a reuse of this font approximate (spec §6.4 "record approximation flags").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceApprox {
    /// False: no font program in the file, so glyph presence is only known from the page.
    pub embedded: bool,
    /// False: no `/ToUnicode`; code to Unicode came from the encoding and glyph names.
    pub to_unicode: bool,
}

pub struct SourceFontFace {
    id: String,
    info: FontInfo,
    /// Covered character -> (code, glyph id).
    map: BTreeMap<char, (u32, u32)>,
    /// The embedded sfnt program, when there is one.
    program: Option<Vec<u8>>,
    approx: SourceApprox,
}

const SFNT: [&[u8]; 3] = [&[0, 1, 0, 0], b"true", b"OTTO"];

impl SourceFontFace {
    /// `glyphs` is the evidence: the glyph items of every page that uses `font` (other fonts'
    /// items are ignored). `Ok(None)` = this font cannot be reused as-is (see the module docs).
    pub fn new<'g>(
        doc: &Document,
        font: ObjectId,
        glyphs: impl IntoIterator<Item = &'g GlyphItem>,
    ) -> Result<Option<Self>, PdfError> {
        let reader = doc.reader();
        let xref = reader.pdf().xref();
        let obj = |id: ObjectId| {
            xref.get::<Object<'_>>(ObjectIdentifier::new(
                id.num as i32,
                i32::from(id.generation),
            ))
        };
        let Some(Object::Dict(fd)) = obj(font) else {
            return Err(PdfError::at(ErrorCode::MalformedPdf, font));
        };
        let Some(info) = FontInfo::read(&fd) else {
            return Ok(None);
        };
        let owner = match info.kind {
            FontKind::Type3 => return Ok(None),
            FontKind::Type0 => {
                if fd
                    .get::<Name<'_>>("Encoding")
                    .map(|n| n.as_str().to_owned())
                    .as_deref()
                    != Some("Identity-H")
                {
                    return Ok(None);
                }
                match fd
                    .get::<Array<'_>>("DescendantFonts")
                    .and_then(|a| a.iter::<Dict<'_>>().next())
                {
                    Some(d) => d,
                    None => return Ok(None),
                }
            }
            _ => fd.clone(),
        };
        let mut program = None;
        let mut embedded = false;
        if let Some(desc) = owner.get::<Dict<'_>>("FontDescriptor") {
            let found = ["FontFile2", "FontFile3", "FontFile"]
                .into_iter()
                .find_map(|k| Some((k, desc.get_raw::<Object<'_>>(k)?)));
            if let Some((key, MaybeRef::Ref(r))) = found {
                embedded = true;
                let pid = object_id(r);
                let Some(Object::Stream(s)) = obj(pid) else {
                    return Err(PdfError::at(ErrorCode::MalformedPdf, pid));
                };
                let mut budget = limits::Budget::new(reader.limits());
                let bytes = limits::decode_stream(&s, pid, reader.limits(), &mut budget)?;
                if key != "FontFile" && SFNT.contains(&&bytes[..bytes.len().min(4)]) {
                    program = Some(bytes.into_owned());
                }
            }
        }
        let glyph_count = program.as_deref().and_then(|p| {
            let m = FontRef::new(p)
                .ok()?
                .metrics(Size::unscaled(), LocationRef::default());
            Some(u32::from(m.glyph_count))
        });
        // An embedded program that is not a loadable sfnt is bare CFF or Type 1: the glyph ids
        // hayro reports are all the evidence there is.
        let max_code = if info.kind == FontKind::Type0 {
            0xFFFF
        } else {
            0xFF
        };
        let mut by_code: BTreeMap<u32, Option<(char, u32)>> = BTreeMap::new();
        for g in glyphs.into_iter().filter(|g| g.font == Some(font)) {
            let (Some(u), Some(gid)) = (g.unicode.as_deref(), g.gid) else {
                continue;
            };
            let mut chars = u.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                continue;
            };
            let sound = (0x20..0xFFFF).contains(&(c as u32))
                && !c.is_control()
                && gid != 0
                && glyph_count.is_none_or(|n| gid < n)
                && g.char_code <= max_code
                && listed_width(&info, g.char_code);
            let new = sound.then_some((c, gid));
            by_code
                .entry(g.char_code)
                .and_modify(|old| {
                    if *old != new {
                        *old = None; // the same code showed two different things
                    }
                })
                .or_insert(new);
        }
        let mut map: BTreeMap<char, (u32, u32)> = BTreeMap::new();
        for (code, v) in by_code {
            if let Some((c, gid)) = v {
                map.entry(c).or_insert((code, gid)); // lowest code first
            }
        }
        Ok(Some(Self {
            id: format!("{} {} R", font.num, font.generation),
            approx: SourceApprox {
                embedded,
                to_unicode: info.has_to_unicode,
            },
            info,
            map,
            program,
        }))
    }

    pub fn approx(&self) -> SourceApprox {
        self.approx
    }

    /// Bytes per character code in a content string: 2 for Type0 (Identity-H), else 1.
    pub fn code_width(&self) -> usize {
        if self.info.kind == FontKind::Type0 {
            2
        } else {
            1
        }
    }

    /// The font's name without the subset tag, for `FontResolution::Embedded.name`.
    pub fn name(&self) -> &str {
        &self.info.name
    }

    /// `FontResolution::Embedded.coversAscii`: every U+0020 to U+007E is covered.
    pub fn covers_ascii(&self) -> bool {
        (' '..='~').all(|c| self.map.contains_key(&c))
    }

    /// The editor face (spec §6.4): a loadable copy of an embedded TrueType/OpenType program with
    /// a cmap rebuilt from the evidence. `None` for no program, CFF/Type 1 and unusable programs.
    /// `fontKey` is a hash of the bytes, so equal evidence gives an equal key.
    pub fn editor_face(&self) -> Option<EditorFace> {
        let pairs: Vec<(u32, u32)> = self.map.iter().map(|(&c, &(_, g))| (c as u32, g)).collect();
        let bytes = editor_face::repair(
            self.program.as_deref()?,
            &self.info.name,
            self.info.bold,
            &pairs,
        )?;
        let mut h = Fnv::new();
        h.field(&bytes);
        Some(EditorFace {
            font_key: format!("src-{}", h.hex()),
            bytes,
        })
    }
}

/// Whether the font states an advance for `code` (never the `/MissingWidth` stand-in).
fn listed_width(info: &FontInfo, code: u32) -> bool {
    if info.kind == FontKind::Type0 {
        info.cid_widths
            .iter()
            .any(|(a, b, _)| (*a..=*b).contains(&code))
            || info.default_width > 0.0
    } else {
        code.checked_sub(info.first_char)
            .is_some_and(|i| (i as usize) < info.widths.len())
    }
}

impl FontFace for SourceFontFace {
    fn id(&self) -> &str {
        &self.id
    }

    fn source(&self) -> RunSource {
        RunSource::Embedded
    }

    fn covers(&self, cluster: &str) -> bool {
        !cluster.is_empty() && cluster.chars().all(|c| self.map.contains_key(&c))
    }

    /// One glyph per character: `gid` is the character code to write, the advance is the font's
    /// listed width, no kerning. A character that is not covered is refused.
    fn shape(&self, text: &str, size: f64) -> Result<Vec<Glyph>, PdfError> {
        let mut at = 0u32;
        text.chars()
            .map(|c| {
                let &(code, _) = self
                    .map
                    .get(&c)
                    .ok_or_else(|| PdfError::new(ErrorCode::EditNoFont))?;
                let end = at + c.len_utf16() as u32;
                let g = Glyph {
                    gid: code,
                    x_advance: self.info.width(code) * size / 1000.0,
                    cluster: at..end,
                };
                at = end;
                Ok(g)
            })
            .collect()
    }

    fn metrics(&self) -> FaceMetrics {
        FaceMetrics {
            ascent: self.info.ascent,
            descent: self.info.descent.abs(),
        }
    }
}
