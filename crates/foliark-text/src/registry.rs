//! Registered fonts (spec §1.4): single-face TrueType/OpenType, parsed with skrifa, shaped by harfrust.
use crate::Fnv;
use crate::layout::{FaceMetrics, FontFace, Glyph};
use foliark_model::{ErrorCode, FontSource, FontStyle, PdfError, RegisteredFont, RunSource};
use harfrust::{Buffer, Direction, Font, Script, ShapeOptions, ShaperFont};
use skrifa::raw::types::Tag;
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider, instance::LocationRef, instance::Size};
use std::collections::BTreeSet;
use std::sync::Arc;

/// A parsed registered font: its public record, cmap coverage and a shaping handle.
pub struct RegisteredFace {
    info: RegisteredFont,
    bytes: Arc<Vec<u8>>,
    font: Font,
    covered: BTreeSet<u32>,
    upem: f64,
    metrics: FaceMetrics,
}

impl RegisteredFace {
    pub fn info(&self) -> &RegisteredFont {
        &self.info
    }

    /// The sfnt exactly as registered (the writer subsets it, spec §6.3).
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl FontFace for RegisteredFace {
    fn id(&self) -> &str {
        &self.info.id
    }

    fn source(&self) -> RunSource {
        RunSource::Registered
    }

    fn covers(&self, cluster: &str) -> bool {
        cluster.chars().all(|c| self.covered.contains(&(c as u32)))
    }

    fn metrics(&self) -> FaceMetrics {
        self.metrics
    }

    fn shape(&self, text: &str, size: f64) -> Result<Vec<Glyph>, PdfError> {
        let font = ShaperFont::new(&self.font);
        let mut buf = Buffer::new();
        buf.push_str(text);
        buf.set_direction(Direction::LeftToRight);
        buf.set_script(Some(Script::LATIN));
        harfrust::shape(&font, &mut buf, ShapeOptions::new())
            .map_err(|_| PdfError::new(ErrorCode::Internal))?;
        // harfrust clusters are UTF-8 byte offsets into `text`; callers get UTF-16.
        let mut utf16 = vec![0u32; text.len() + 1];
        let mut n = 0u32;
        for (i, c) in text.char_indices() {
            utf16[i] = n;
            n += c.len_utf16() as u32;
        }
        utf16[text.len()] = n;
        let (infos, pos) = (buf.glyph_infos(), buf.glyph_positions());
        let mut out: Vec<Glyph> = Vec::with_capacity(infos.len());
        let mut end = n;
        for (info, p) in infos.iter().zip(pos).rev() {
            let start = utf16[info.cluster as usize];
            // A cluster spans up to the next distinct (greater) cluster start.
            if let Some(next) = out.last()
                && next.cluster.start > start
            {
                end = next.cluster.start;
            }
            out.push(Glyph {
                gid: info.glyph_id,
                x_advance: f64::from(p.x_advance) * size / self.upem,
                cluster: start..end,
            });
        }
        out.reverse();
        Ok(out)
    }
}

/// Fonts are per client and shared by all documents; every successful `register` bumps `version`.
#[derive(Default)]
pub struct Registry {
    faces: Vec<RegisteredFace>,
    version: u32,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// `fontRegistryVersion`: part of every plan fingerprint.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Bytes of every registered face (`maxRegisteredFontBytes` counts them).
    pub fn total_bytes(&self) -> u64 {
        self.faces.iter().map(|f| f.bytes.len() as u64).sum()
    }

    pub fn get(&self, id: &str) -> Option<&RegisteredFace> {
        self.faces.iter().find(|f| f.info.id == id)
    }

    /// Case-insensitive family; the closest weight/style wins (exact first, then weight, then style).
    pub fn lookup(&self, family: &str, bold: bool, italic: bool) -> Option<&RegisteredFace> {
        let want = (
            if bold { 700 } else { 400 },
            if italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
        );
        self.faces
            .iter()
            .filter(|f| f.info.family.eq_ignore_ascii_case(family))
            .min_by_key(|f| (f.info.weight != want.0, f.info.style != want.1))
    }

    /// Registers one single-face glyf or CFF font; anything else is `FONT_UNSUPPORTED`.
    /// Registering identical bytes under the same family/weight/style again replaces that entry.
    pub fn register(&mut self, src: FontSource) -> Result<RegisteredFont, PdfError> {
        if src.family.trim().is_empty() || !matches!(src.weight, 400 | 700) {
            return Err(PdfError::new(ErrorCode::InvalidArgument));
        }
        let unsupported = || PdfError::new(ErrorCode::FontUnsupported);
        // Collections (`ttcf`), WOFF, Type1 and junk all fail the sfnt magic.
        if !matches!(src.bytes.get(..4), Some([0, 1, 0, 0] | b"OTTO" | b"true")) {
            return Err(unsupported());
        }
        let font_ref = FontRef::new(&src.bytes).map_err(|_| unsupported())?;
        let has = |t: &[u8; 4]| font_ref.table_data(Tag::new(t)).is_some();
        if !(has(b"glyf") || has(b"CFF ")) {
            return Err(unsupported());
        }
        let covered: BTreeSet<u32> = font_ref
            .charmap()
            .mappings()
            .filter(|(_, g)| g.to_u32() != 0)
            .map(|(c, _)| c)
            .collect();
        let m = font_ref.metrics(Size::unscaled(), LocationRef::default());
        if covered.is_empty() || m.units_per_em == 0 {
            return Err(unsupported());
        }
        let upem = f64::from(m.units_per_em);
        let postscript_name = font_ref
            .localized_strings(StringId::POSTSCRIPT_NAME)
            .english_or_first()
            .map(|s| s.chars().collect::<String>())
            .unwrap_or_else(|| src.family.replace(' ', ""));
        let glyph_count = u32::from(m.glyph_count);
        let id = {
            let mut h = Fnv::new();
            h.field(&src.bytes)
                .field(src.family.as_bytes())
                .field(&src.weight.to_le_bytes())
                .field(&[src.style as u8]);
            format!("font-{}", h.hex())
        };
        let bytes = Arc::new(src.bytes);
        let shared: Arc<dyn AsRef<[u8]> + Send + Sync> = bytes.clone();
        let font = Font::new(shared, 0).ok_or_else(unsupported)?;
        let info = RegisteredFont {
            id,
            family: src.family,
            weight: src.weight,
            style: src.style,
            postscript_name,
            glyph_count,
        };
        let face = RegisteredFace {
            info: info.clone(),
            bytes,
            font,
            covered,
            upem,
            metrics: FaceMetrics {
                ascent: f64::from(m.ascent) / upem,
                descent: f64::from(m.descent).abs() / upem,
            },
        };
        match self.faces.iter_mut().find(|f| f.info.id == info.id) {
            Some(slot) => *slot = face,
            None => self.faces.push(face),
        }
        self.version += 1;
        Ok(info)
    }
}
