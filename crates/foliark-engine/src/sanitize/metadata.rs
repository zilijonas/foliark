//! The Info fields kept by `policy.metadata: 'keep'` (ADR 0016, native export only). Only the
//! eight standard keys can come back, read from hayro's parsed `Metadata` of the working copy,
//! never from the raw objects:
//!
//! 1. a text field is decoded (UTF-16BE or UTF-8 with a BOM, else PDFDocEncoding), stripped of
//!    language escapes, controls and bidi controls, NFC, trimmed; empty or over [`MAX_CHARS`]
//!    characters drops it;
//! 2. a date is re-emitted in the `D:YYYYMMDDHHmmSS` grammar from the parsed value; one that is not
//!    a real date drops it;
//! 3. a text field holding a token of text a redaction or an edit took out is dropped, compared in
//!    its plain and its letters-and-digits-only form (the rule of ADR 0014, `links::tokens_of`).
//!
//! The writer emits the survivors as one fresh dictionary; XMP is never written.
use super::links::{MIN_TOKEN, normal};
use crate::Document;
use crate::reader;
use foliark_model::{ExtractOptions, PdfError};
use hayro::hayro_syntax::object::{DateTime, Dict};
use std::collections::BTreeSet;
use unicode_normalization::UnicodeNormalization;

/// Longest text value kept, in characters.
const MAX_CHARS: usize = 1024;

/// PDFDocEncoding of 0x18..=0x1F, 0x80..=0x9E and 0xA0 (PDF 32000-1 Annex D.2); the other
/// codes are Latin-1 or undefined (0x7F, 0x9F, 0xAD).
const PDFDOC_LOW: [char; 8] = [
    '\u{2d8}', '\u{2c7}', '\u{2c6}', '\u{2d9}', '\u{2dd}', '\u{2db}', '\u{2da}', '\u{2dc}',
];
const PDFDOC_HIGH: [char; 31] = [
    '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{192}', '\u{2044}',
    '\u{2039}', '\u{203a}', '\u{2212}', '\u{2030}', '\u{201e}', '\u{201c}', '\u{201d}', '\u{2018}',
    '\u{2019}', '\u{201a}', '\u{2122}', '\u{fb01}', '\u{fb02}', '\u{141}', '\u{152}', '\u{160}',
    '\u{178}', '\u{17d}', '\u{131}', '\u{142}', '\u{153}', '\u{161}', '\u{17e}',
];

/// What `keep` decided: the entries to write and how many keys the input's Info held.
#[derive(Debug, Default)]
pub struct Kept {
    pub entries: Vec<(&'static str, Vec<u8>)>,
    pub input_keys: u32,
}

impl Kept {
    /// Info keys of the input that are not written (they count as removed metadata).
    pub fn dropped(&self) -> u32 {
        self.input_keys.saturating_sub(self.entries.len() as u32)
    }

    pub fn names(&self) -> Vec<String> {
        self.entries.iter().map(|(k, _)| (*k).to_owned()).collect()
    }
}

fn pdfdoc(b: &[u8]) -> String {
    b.iter()
        .filter_map(|&c| match c {
            0x18..=0x1f => Some(PDFDOC_LOW[usize::from(c) - 0x18]),
            0x80..=0x9e => Some(PDFDOC_HIGH[usize::from(c) - 0x80]),
            0x7f | 0x9f | 0xad => None,
            0xa0 => Some('\u{20ac}'),
            c => Some(char::from(c)),
        })
        .collect()
}

/// U+001B ... U+001B runs (language tags) removed; an unterminated one runs to the end.
fn without_language_escapes(s: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in s.chars() {
        match (c, inside) {
            ('\u{1b}', _) => inside = !inside,
            (c, false) => out.push(c),
            _ => {}
        }
    }
    out
}

/// Characters that draw nothing: controls (Cc), format characters (Cf: bidi marks, zero-width
/// joiners and spaces, soft hyphen, BOM, tags U+E0000-E007F, ...), private use (Co),
/// noncharacters and variation selectors. They could carry or split a redacted token unseen,
/// and `objects.py` rejects Cc and Cf in Info text. Unassigned code points (Cn) other than the
/// noncharacters pass: there is no Unicode table here.
fn invisible(c: char) -> bool {
    let u = c as u32;
    c.is_control()
        || matches!(u,
            0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x890..=0x891 | 0x8e2 | 0x180e
            | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x2064 | 0x2066..=0x206f | 0xfeff
            | 0xfff9..=0xfffb | 0x110bd | 0x110cd | 0x13430..=0x1343f | 0x1bca0..=0x1bca3
            | 0x1d173..=0x1d17a | 0xe0000..=0xe007f)
        // Variation selectors.
        || matches!(u, 0x180b..=0x180d | 0x180f | 0xfe00..=0xfe0f | 0xe0100..=0xe01ef)
        // Private use.
        || matches!(u, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
        // Noncharacters.
        || matches!(u, 0xfdd0..=0xfdef)
        || u & 0xfffe == 0xfffe
}

/// The normalized text of a raw Info string, or `None` when it is empty, too long or does not
/// decode.
fn text(raw: &[u8]) -> Option<String> {
    let decoded = if let Some(be) = raw.strip_prefix(&[0xfe, 0xff]) {
        let units: Vec<u16> = be
            .chunks(2)
            .map(|p| <[u8; 2]>::try_from(p).map(u16::from_be_bytes))
            .collect::<Result<_, _>>()
            .ok()?;
        without_language_escapes(&String::from_utf16(&units).ok()?)
    } else if let Some(utf8) = raw.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        without_language_escapes(std::str::from_utf8(utf8).ok()?)
    } else {
        pdfdoc(raw)
    };
    // Stripped before NFC: a mark left next to its base once a joiner goes composes with it.
    let clean: String = decoded
        .chars()
        .filter(|c| !invisible(*c))
        .collect::<String>()
        .nfc()
        .collect();
    let clean = clean.trim();
    (!clean.is_empty() && clean.chars().count() <= MAX_CHARS).then(|| clean.to_owned())
}

/// A printable ASCII value is written as it is; anything else as UTF-16BE with a BOM.
fn encoded(s: &str) -> Vec<u8> {
    if s.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
        return s.as_bytes().to_vec();
    }
    let mut out = vec![0xfe, 0xff];
    out.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
    out
}

/// `D:YYYYMMDDHHmmSS` plus the offset, or `None` for a date that does not exist. The parser
/// drops the sign of an offset under one hour (`-00'30'` reads as `+00'30'`), so one is refused.
fn date(d: &DateTime) -> Option<Vec<u8>> {
    let leap =
        d.year.is_multiple_of(4) && (!d.year.is_multiple_of(100) || d.year.is_multiple_of(400));
    let days = match d.month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if d.day > days || d.hour > 23 || d.minute > 59 || d.second > 59 || d.utc_offset_minute > 59 {
        return None;
    }
    let zone = match (d.utc_offset_hour, d.utc_offset_minute) {
        (0, 0) => "Z".to_owned(),
        (0, _) => return None,
        (h, m) => format!(
            "{}{:02}'{m:02}'",
            if h < 0 { '-' } else { '+' },
            h.unsigned_abs()
        ),
    };
    Some(
        format!(
            "D:{:04}{:02}{:02}{:02}{:02}{:02}{zone}",
            d.year, d.month, d.day, d.hour, d.minute, d.second
        )
        .into_bytes(),
    )
}

fn alnum(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Whether `value` (normalized text) holds a token, plain or letters-and-digits only.
fn leaks(value: &str, tokens: &BTreeSet<String>) -> bool {
    let lower = normal(value);
    let squashed = alnum(&lower);
    tokens.iter().any(|t| {
        let s = alnum(t);
        let forms = [
            Some(t.as_str()),
            (s.chars().count() >= MIN_TOKEN).then_some(s.as_str()),
        ];
        forms
            .into_iter()
            .flatten()
            .any(|t| lower.contains(t) || squashed.contains(t))
    })
}

/// The fields of `source`'s Info that `policy.metadata: 'keep'` writes back, in the order of
/// ADR 0016. `tokens` is called at most once, only when a text field needs checking.
pub fn keep(
    source: &reader::Document,
    tokens: impl FnOnce() -> Result<BTreeSet<String>, PdfError>,
) -> Result<Kept, PdfError> {
    let xref = source.pdf().xref();
    let input_keys = xref
        .info_id()
        .and_then(|id| xref.get::<Dict<'_>>(id))
        .map_or(0, |d| d.keys().count() as u32);
    let m = source.pdf().metadata();
    let fields = [
        ("Title", &m.title),
        ("Author", &m.author),
        ("Subject", &m.subject),
        ("Keywords", &m.keywords),
        ("Creator", &m.creator),
        ("Producer", &m.producer),
    ];
    let texts: Vec<_> = fields
        .into_iter()
        .filter_map(|(k, v)| Some((k, text(v.as_deref()?)?)))
        .collect();
    let removed = if texts.is_empty() {
        BTreeSet::new()
    } else {
        tokens()?
    };
    let mut entries: Vec<(&'static str, Vec<u8>)> = texts
        .into_iter()
        .filter(|(_, v)| !leaks(v, &removed))
        .map(|(k, v)| (k, encoded(&v)))
        .collect();
    // Dates sit after the text keys (CreationDate, ModDate); Producer is the last text key.
    for (k, d) in [
        ("CreationDate", &m.creation_date),
        ("ModDate", &m.modification_date),
    ] {
        if let Some(v) = d.as_ref().and_then(date) {
            entries.push((k, v));
        }
    }
    Ok(Kept {
        entries,
        input_keys,
    })
}

/// The words of text the edits took out: those of the edited pages in `source` that no edited
/// page of `working` still shows (a word the edit kept is not lost). Words are whitespace
/// pieces of at least [`MIN_TOKEN`] characters without the punctuation around them, normalized
/// like the tokens of a redaction.
pub fn edited_away(
    source: &Document,
    working: &Document,
    pages: &BTreeSet<usize>,
) -> Result<BTreeSet<String>, PdfError> {
    let opts = ExtractOptions {
        chars: false,
        include_ocg_hidden: true,
    };
    let words = |doc: &Document| -> Result<BTreeSet<String>, PdfError> {
        let mut out = BTreeSet::new();
        for &p in pages {
            let page = doc.extract_text(p, &opts)?;
            out.extend(
                page.text
                    .split_whitespace()
                    .map(|w| normal(w.trim_matches(|c: char| !c.is_alphanumeric())))
                    .filter(|w| w.chars().count() >= MIN_TOKEN),
            );
        }
        Ok(out)
    };
    let kept = words(working)?;
    Ok(words(source)?.difference(&kept).cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_decodes_each_form_and_strips_what_is_not_text() {
        assert_eq!(text(b"  Plain \x07title ").as_deref(), Some("Plain title"));
        assert_eq!(
            text(b"\x84\x80 \xe9").as_deref(),
            Some("\u{2014}\u{2022} \u{e9}")
        );
        // UTF-16BE: language escape, a bidi control and a combining accent (NFC joins it).
        let u: Vec<u8> = [0xfeffu16, 0x1b, 0x65, 0x6e, 0x1b, 0x202e, 0x65, 0x301]
            .iter()
            .flat_map(|u| u.to_be_bytes())
            .collect();
        assert_eq!(text(&u).as_deref(), Some("\u{e9}"));
        assert_eq!(
            text(b"\xef\xbb\xbfcaf\xc3\xa9").as_deref(),
            Some("caf\u{e9}")
        );
        assert_eq!(text(b"\xfe\xff\xd8\x00"), None); // lone surrogate
        assert_eq!(text(b" \x01 "), None);
        assert_eq!(text(&[b'a'; MAX_CHARS + 1]), None);
        assert!(text(&[b'a'; MAX_CHARS]).is_some());
    }

    #[test]
    fn encoded_is_ascii_or_utf16_with_bom() {
        assert_eq!(encoded("Report 1"), b"Report 1");
        assert_eq!(encoded("\u{e9}"), [0xfe, 0xff, 0x00, 0xe9]);
    }

    #[test]
    fn dates_are_rewritten_or_refused() {
        let d = |month, day, h, m| DateTime {
            year: 2024,
            month,
            day,
            hour: 1,
            minute: 2,
            second: 3,
            utc_offset_hour: h,
            utc_offset_minute: m,
        };
        assert_eq!(date(&d(2, 29, 0, 0)).unwrap(), b"D:20240229010203Z");
        assert_eq!(date(&d(7, 4, -5, 30)).unwrap(), b"D:20240704010203-05'30'");
        assert_eq!(date(&d(7, 4, 2, 0)).unwrap(), b"D:20240704010203+02'00'");
        assert_eq!(date(&d(4, 31, 0, 0)), None);
        assert_eq!(date(&d(7, 4, 0, 30)), None);
    }

    #[test]
    fn leak_compares_plain_and_squashed_forms() {
        let t: BTreeSet<String> = ["marta jones", "martajones", "acme-9"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert!(leaks("Contract MARTA JONES", &t));
        assert!(leaks("Contract Marta-Jones", &t));
        assert!(leaks("project ACME 9", &t));
        assert!(!leaks("Contract with Marta", &t));
    }
}
