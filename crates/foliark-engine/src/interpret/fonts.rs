//! What the semantic model records about a font dictionary (spec §3.1, §3.2).
use hayro::hayro_syntax::object::{Array, Dict, Name};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontKind {
    Type1,
    TrueType,
    Type0,
    Type3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FontInfo {
    /// /BaseFont without the `ABCDEF+` subset tag.
    pub name: String,
    pub kind: FontKind,
    pub bold: bool,
    pub italic: bool,
    pub serif: bool,
    pub mono: bool,
    /// Em fractions for the §3.1 quad: descriptor /Ascent and /Descent over 1000, 0.8 / -0.2
    /// when the descriptor has neither, scaled to a height of 1 when taller.
    pub ascent: f64,
    pub descent: f64,
    pub has_to_unicode: bool,
    /// Simple fonts: `/Widths` (thousandths of an em) from `first_char`. Type0: empty, the
    /// `/W` array is not read here.
    pub first_char: u32,
    pub widths: Vec<f64>,
    /// `/MissingWidth` (simple fonts) or `/DW` (CID fonts, default 1000).
    pub default_width: f64,
}

const FIXED_PITCH: u32 = 1;
const SERIF: u32 = 1 << 1;
const ITALIC: u32 = 1 << 6;
const FORCE_BOLD: u32 = 1 << 18;

impl FontInfo {
    /// `None` when the dictionary is not a font this model knows.
    pub fn read(font: &Dict<'_>) -> Option<Self> {
        let kind = match font.get::<Name<'_>>("Subtype")?.as_str() {
            "Type1" | "MMType1" => FontKind::Type1,
            "TrueType" => FontKind::TrueType,
            "Type0" => FontKind::Type0,
            "Type3" => FontKind::Type3,
            _ => return None,
        };
        let cid = (kind == FontKind::Type0)
            .then(|| {
                font.get::<Array<'_>>("DescendantFonts")?
                    .iter::<Dict<'_>>()
                    .next()
            })
            .flatten();
        let desc = cid
            .as_ref()
            .unwrap_or(font)
            .get::<Dict<'_>>("FontDescriptor");
        let num = |key: &str| desc.as_ref().and_then(|d| d.get::<f64>(key));
        let name = font
            .get::<Name<'_>>("BaseFont")
            .map(|n| n.as_str().to_owned())
            .unwrap_or_default();
        let name = strip_subset_tag(&name).to_owned();
        let lower = name.to_ascii_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
        let flags = num("Flags").map_or(0, |f| f as u32);
        let (a, d) = match (num("Ascent"), num("Descent")) {
            (Some(a), Some(d)) if a != 0.0 || d != 0.0 => (a / 1000.0, d / 1000.0),
            _ => (0.8, -0.2),
        };
        let height = a - d;
        let (ascent, descent) = if height > 1.0 {
            (a / height, d / height)
        } else {
            (a, d)
        };
        Some(Self {
            bold: flags & FORCE_BOLD != 0
                || num("FontWeight").is_some_and(|w| w >= 600.0)
                || has(&["bold", "black", "heavy", "semibold", "demi"]),
            italic: flags & ITALIC != 0
                || num("ItalicAngle").is_some_and(|a| a != 0.0)
                || has(&["italic", "oblique"]),
            serif: flags & SERIF != 0,
            mono: flags & FIXED_PITCH != 0,
            name,
            kind,
            ascent,
            descent,
            has_to_unicode: font.contains_key("ToUnicode"),
            first_char: font.get::<u32>("FirstChar").unwrap_or(0),
            widths: font
                .get::<Array<'_>>("Widths")
                .map(|w| w.iter::<f64>().collect())
                .unwrap_or_default(),
            default_width: match &cid {
                Some(c) => c.get::<f64>("DW").unwrap_or(1000.0),
                None => num("MissingWidth").unwrap_or(0.0),
            },
        })
    }
}

/// `ABCDEF+Name` is `Name`.
fn strip_subset_tag(name: &str) -> &str {
    match name.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => name,
    }
}
