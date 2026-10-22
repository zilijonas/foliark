//! What the semantic model records about a font dictionary (spec §3.1, §3.2).
use hayro::hayro_syntax::object::{Array, Dict, Name, Object, Stream};
use skrifa::{FontRef, MetadataProvider, instance::LocationRef, instance::Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontKind {
    Type1,
    TrueType,
    Type0,
    Type3,
}

/// Where [`FontInfo::ascent`] and [`FontInfo::descent`] came from, best first (spec §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricsSource {
    Descriptor,
    /// hhea / OS/2 of the embedded TrueType or OpenType program.
    FontProgram,
    /// Descriptor `/FontBBox` (Type3: the font bbox through `/FontMatrix`).
    FontBBox,
    /// No metric anywhere: 0.8 / -0.2.
    Default,
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
    /// Em fractions for the §3.1 quad, from the first source that has them, scaled to a height
    /// of 1 when taller.
    pub ascent: f64,
    pub descent: f64,
    pub metrics: MetricsSource,
    pub has_to_unicode: bool,
    /// Simple fonts: `/Widths` (thousandths of an em) from `first_char`. Type0: empty, the
    /// `/W` array is not read here.
    pub first_char: u32,
    pub widths: Vec<f64>,
    /// `/MissingWidth` (simple fonts) or `/DW` (CID fonts, default 1000).
    pub default_width: f64,
    /// CID fonts: `/W` as inclusive `(first, last, width)` ranges, thousandths of an em.
    pub cid_widths: Vec<(u32, u32, f64)>,
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
        let ((a, d), metrics) = vertical_metrics(font, desc.as_ref(), kind);
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
            metrics,
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
            cid_widths: cid.as_ref().map(cid_widths).unwrap_or_default(),
        })
    }

    /// Width in thousandths of an em of a simple-font char code or a CID (an identity CMap
    /// code is its CID); the default width when the font lists none.
    pub fn width(&self, code_or_cid: u32) -> f64 {
        if self.kind == FontKind::Type0 {
            return self
                .cid_widths
                .iter()
                .find(|(first, last, _)| (*first..=*last).contains(&code_or_cid))
                .map_or(self.default_width, |r| r.2);
        }
        code_or_cid
            .checked_sub(self.first_char)
            .and_then(|i| self.widths.get(i as usize))
            .copied()
            .unwrap_or(self.default_width)
    }
}

/// `(ascent, descent)` in em, and where they came from.
fn vertical_metrics(
    font: &Dict<'_>,
    desc: Option<&Dict<'_>>,
    kind: FontKind,
) -> ((f64, f64), MetricsSource) {
    if kind == FontKind::Type3 {
        let bbox = font.get::<[f64; 4]>("FontBBox");
        let scale = font.get::<[f64; 6]>("FontMatrix").map_or(0.001, |m| m[3]);
        return match bbox {
            Some(b) if b[1] != b[3] => ((b[3] * scale, b[1] * scale), MetricsSource::FontBBox),
            _ => ((0.8, -0.2), MetricsSource::Default),
        };
    }
    if let Some(d) = desc {
        let (a, ds) = (d.get::<f64>("Ascent"), d.get::<f64>("Descent"));
        if let (Some(a), Some(ds)) = (a, ds)
            && (a != 0.0 || ds != 0.0)
        {
            return ((a / 1000.0, ds / 1000.0), MetricsSource::Descriptor);
        }
        if let Some(m) = ["FontFile2", "FontFile3"]
            .into_iter()
            .find_map(|k| d.get::<Stream<'_>>(k))
            .and_then(|s| program_metrics(&s))
        {
            return (m, MetricsSource::FontProgram);
        }
        if let Some(b) = d.get::<[f64; 4]>("FontBBox")
            && b[1] != b[3]
        {
            return ((b[3] / 1000.0, b[1] / 1000.0), MetricsSource::FontBBox);
        }
    }
    ((0.8, -0.2), MetricsSource::Default)
}

/// hhea / OS/2 ascent and descent of an sfnt program (bare CFF and Type1 have none).
fn program_metrics(stream: &Stream<'_>) -> Option<(f64, f64)> {
    let data = stream.decoded().ok()?;
    let m = FontRef::new(&data)
        .ok()?
        .metrics(Size::unscaled(), LocationRef::default());
    let upem = f64::from(m.units_per_em);
    (upem > 0.0 && (m.ascent != 0.0 || m.descent != 0.0))
        .then(|| (f64::from(m.ascent) / upem, f64::from(m.descent) / upem))
}

/// A CID font's `/W`: `c [w1 w2 ...]` and `first last w`, both forms mixed.
fn cid_widths(cid: &Dict<'_>) -> Vec<(u32, u32, f64)> {
    let mut out = Vec::new();
    let Some(w) = cid.get::<Array<'_>>("W") else {
        return out;
    };
    let num = |o: Option<Object<'_>>| match o {
        Some(Object::Number(n)) => Some(n.as_f64()),
        _ => None,
    };
    let mut items = w.iter::<Object<'_>>();
    while let Some(first) = num(items.next()) {
        let first = first as u32;
        match items.next() {
            Some(Object::Array(list)) => {
                for (i, width) in list.iter::<f64>().enumerate() {
                    out.push((first + i as u32, first + i as u32, width));
                }
            }
            Some(Object::Number(last)) => match num(items.next()) {
                Some(width) => out.push((first, last.as_f64() as u32, width)),
                None => break,
            },
            _ => break,
        }
    }
    out
}

/// `ABCDEF+Name` is `Name`.
fn strip_subset_tag(name: &str) -> &str {
    match name.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hayro::hayro_syntax::Pdf;
    use hayro::hayro_syntax::object::ObjRef;
    use pdf_writer::{Finish, Name as N, Pdf as Out, Ref};

    #[test]
    fn type0_widths_read_both_w_forms_then_dw() {
        let mut pdf = Out::new();
        pdf.catalog(Ref::new(1)).pages(Ref::new(2));
        pdf.pages(Ref::new(2)).kids([]).count(0);
        let mut cid = pdf.indirect(Ref::new(3)).dict();
        cid.pair(N(b"Subtype"), N(b"CIDFontType2"));
        cid.pair(N(b"DW"), 500);
        {
            let mut w = cid.insert(N(b"W")).array();
            w.item(1);
            w.push().array().items([300, 400]);
            w.items([10, 20, 250]);
        }
        cid.finish();
        let mut font = pdf.indirect(Ref::new(4)).dict();
        font.pair(N(b"Subtype"), N(b"Type0"));
        font.pair(N(b"BaseFont"), N(b"ABCDEF+Test"));
        font.insert(N(b"DescendantFonts")).array().item(Ref::new(3));
        font.finish();
        let pdf = Pdf::new(pdf.finish()).unwrap();
        let dict: Dict<'_> = pdf.xref().get(ObjRef::new(4, 0).into()).unwrap();
        let info = FontInfo::read(&dict).unwrap();
        let widths: Vec<_> = [0, 1, 2, 3, 10, 15, 20, 21].map(|c| info.width(c)).into();
        assert_eq!(
            widths,
            [500.0, 300.0, 400.0, 500.0, 250.0, 250.0, 250.0, 500.0]
        );
        assert_eq!(info.metrics, MetricsSource::Default);
    }
}
