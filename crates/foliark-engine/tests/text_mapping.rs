//! Text extraction (spec §3): UTF-16 offsets, clusters, grouping and order, plus parity with
//! PyMuPDF `get_text("words", sort=True)` on the committed native-text fixtures
//! (`tests/oracles/dump_words.py` wrote `tests/data/pymupdf-words/*.json`).
mod common;
use common::{Font, build, close, slice16, texts, width};
use foliark_engine::Document;
use foliark_model::ExtractOptions;
use std::path::PathBuf;

const CHARS: ExtractOptions = ExtractOptions {
    chars: true,
    include_ocg_hidden: false,
};

// ---- parity with PyMuPDF -------------------------------------------------------------------------

/// Largest accepted difference of any word box coordinate, in points. Spec §3.6 says 0.75 for
/// the small-glyph-height model; PyMuPDF measures glyph height with Nimbus/embedded metrics
/// where a non-embedded base-14 font here has no descriptor and falls back to 0.8 / -0.2.
const BOX_TOLERANCE: f64 = 1.0;

/// Intentional deviations from PyMuPDF (fixture, PyMuPDF text, our text). Same word position (the right edge follows the text), other text.
/// 1. `/ActualText`: MuPDF replaces the marked glyphs' text with the ActualText string; a
///    cluster here is the shown glyph and its own Unicode (redaction and selection act on
///    what is drawn).
///
/// Not exercised by a fixture but also intended: a ligature glyph stays ONE cluster with its
/// ToUnicode string (no splitting); an overprinted string is its own line AND block (spec §3.3
/// needs the baseline to advance, PyMuPDF keeps it in the block); block numbers of
/// multi-column pages differ (spec §3.6).
const DEVIATIONS: [(&str, &str, &str); 1] =
    [("hddn/actualtext.pdf", "SECRET_ACTUALTEXT", "PUBLIC_GLYPHS")];

fn parity_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/pymupdf-words")
}

#[derive(Default)]
struct Parity {
    words: usize,
    matched: usize,
    deviations: usize,
    max_delta: f64,
    notes: Vec<String>,
}

fn compare(fixture: &str, json: &serde_json::Value, doc: &Document) -> Parity {
    let mut p = Parity::default();
    for (i, page) in json["pages"].as_array().unwrap().iter().enumerate() {
        let expected = page["words"].as_array().unwrap();
        let got = doc
            .extract_text(i, &ExtractOptions::default())
            .unwrap_or_else(|e| panic!("{fixture} p{i}: {e:?}"));
        p.words += expected.len().max(got.words.len());
        for (n, (e, w)) in expected.iter().zip(&got.words).enumerate() {
            let b = e["box"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect::<Vec<_>>();
            let mine = [
                w.bounds.x,
                w.bounds.y,
                w.bounds.x + w.bounds.width,
                w.bounds.y + w.bounds.height,
            ];
            let delta = b
                .iter()
                .zip(mine)
                .map(|(a, c)| (a - c).abs())
                .fold(0.0, f64::max);
            p.max_delta = p.max_delta.max(delta);
            let text = e["text"].as_str().unwrap();
            if text == w.text && delta <= BOX_TOLERANCE {
                p.matched += 1;
            } else if DEVIATIONS.contains(&(fixture, text, w.text.as_str()))
                && [0, 1, 3]
                    .iter()
                    .all(|&k| (b[k] - mine[k]).abs() <= BOX_TOLERANCE)
            {
                // The text differs, so the right edge does; left, top and bottom still agree.
                p.deviations += 1;
            } else {
                p.notes.push(format!(
                    "{fixture} p{i} word {n}: {:?} vs {:?} delta {delta:.2}",
                    e["text"], w.text
                ));
            }
        }
        if expected.len() != got.words.len() {
            p.notes.push(format!(
                "{fixture} p{i}: {} words vs {}",
                expected.len(),
                got.words.len()
            ));
        }
    }
    p
}

#[test]
fn words_match_pymupdf_on_native_fixtures() {
    let mut files: Vec<_> = std::fs::read_dir(parity_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    assert!(files.len() >= 20, "oracle dumps missing");
    let mut report = Vec::new();
    for f in files {
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
        let fixture = json["fixture"].as_str().unwrap();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(fixture);
        let doc = Document::open(std::fs::read(path).unwrap(), None)
            .unwrap_or_else(|e| panic!("{fixture}: {e:?}"));
        let p = compare(fixture, &json, &doc);
        report.push(format!(
            "{fixture}: {}/{} words identical ({:.0}%), {} documented deviation, max box delta {:.3} pt",
            p.matched,
            p.words,
            100.0 * p.matched as f64 / p.words.max(1) as f64,
            p.deviations,
            p.max_delta
        ));
        assert_eq!(p.matched + p.deviations, p.words, "{}", p.notes.join("\n"));
    }
    println!("{}", report.join("\n"));
}

// ---- offsets, clusters, grouping -----------------------------------------------------------------

#[test]
fn utf16_offsets_slice_exactly_for_surrogates_ligatures_and_multi_codepoint_clusters() {
    let font = Font {
        to_unicode: vec![(1, "\u{1F600}"), (2, "fi"), (3, "\u{FB01}"), (4, "\u{105}")],
        unnamed: false,
    };
    let doc = build(
        &["BT /F1 10 Tf 1 0 0 1 20 300 Tm (A\\001B \\002 \\003\\004) Tj ET"],
        &font,
        0,
    );
    let page = doc.extract_text(0, &CHARS).unwrap();
    assert_eq!(page.text, "A\u{1F600}B fi \u{FB01}\u{105}");
    assert_eq!(texts(&page), ["A\u{1F600}B", "fi", "\u{FB01}\u{105}"]);
    for w in &page.words {
        assert_eq!(slice16(&page.text, w.start, w.end), w.text);
    }
    let clusters = page.clusters.as_ref().unwrap();
    let got: Vec<_> = clusters
        .iter()
        .map(|c| {
            (
                slice16(&page.text, c.start, c.end),
                c.end - c.start,
                c.flags,
            )
        })
        .collect();
    // The emoji is one cluster of two UTF-16 units, "fi" one multi-codepoint cluster (flag 4),
    // U+FB01 one single-codepoint cluster; each is exactly one glyph wide.
    assert_eq!(
        got,
        [
            ("A".into(), 1, 0),
            ("\u{1F600}".into(), 2, 0),
            ("B".into(), 1, 0),
            ("fi".into(), 2, 4),
            ("\u{FB01}".into(), 1, 0),
            ("\u{105}".into(), 1, 0)
        ]
    );
    assert!(clusters.iter().all(|c| close(width(&c.quad), 5.0)));
    assert_eq!(page.stats.visible_clusters, 6);
}

#[test]
fn lt_lv_et_pl_letters_keep_offsets_and_separately_drawn_marks_are_their_own_cluster() {
    let words = ["ąčęėįšųūž", "āēģīķļņ", "õäöü", "ąćęłńóśźż"];
    let table: Vec<(u8, String)> = words
        .iter()
        .flat_map(|w| w.chars())
        .enumerate()
        .map(|(i, c)| (i as u8 + 128, c.to_string()))
        .collect();
    let font = Font {
        to_unicode: table
            .iter()
            .map(|(c, s)| (*c, &*Box::leak(s.clone().into_boxed_str())))
            .chain([(100u8, "\u{328}")])
            .collect(),
        unnamed: false,
    };
    let mut shown = String::new();
    let mut code = 128;
    for w in words {
        shown.push('(');
        for _ in w.chars() {
            shown.push_str(&format!("\\{code:03o}"));
            code += 1;
        }
        shown.push_str(") Tj ( ) Tj ");
    }
    // Base "a" then U+0328 drawn 0.4 em back (a TJ move): a decomposed a-ogonek.
    let content = format!("BT /F1 10 Tf 1 0 0 1 20 300 Tm {shown}[(a) 400 (d)] TJ ET");
    let page = build(&[&content], &font, 0)
        .extract_text(0, &CHARS)
        .unwrap();
    assert_eq!(page.text, "ąčęėįšųūž āēģīķļņ õäöü ąćęłńóśźż a\u{328}");
    assert_eq!(page.words.len(), 5);
    for w in &page.words {
        assert_eq!(slice16(&page.text, w.start, w.end), w.text);
        // Precomposed letters are one UTF-16 unit and one cluster each.
        if w.text.chars().all(|c| c.len_utf16() == 1) {
            assert_eq!(w.end - w.start, w.text.chars().count() as u32);
        }
    }
    let marked = &page.words[4];
    let clusters: Vec<_> = page
        .clusters
        .as_ref()
        .unwrap()
        .iter()
        .filter(|c| c.start >= marked.start)
        .collect();
    assert_eq!(
        clusters.len(),
        2,
        "base and mark are two clusters in one word"
    );
    assert!(
        clusters[1].quad.0[0].x < clusters[0].quad.0[1].x,
        "the mark overlaps its base"
    );
}

#[test]
fn overprinted_text_is_kept_in_content_order_not_merged() {
    let content = "BT /F1 10 Tf 1 0 0 1 20 300 Tm (Hello) Tj 1 0 0 1 20 300 Tm (Hello) Tj ET";
    let page = build(&[content], &Font::default(), 0)
        .extract_text(0, &CHARS)
        .unwrap();
    assert_eq!(texts(&page), ["Hello", "Hello"]);
    assert_eq!(page.words[0].bounds, page.words[1].bounds);
    assert_eq!(page.text, "Hello Hello");
    // The second drawing does not advance the baseline, so it is its own block (see DEVIATIONS).
    assert_ne!(page.words[0].block_index, page.words[1].block_index);
}

#[test]
fn glyphs_without_unicode_are_replacement_characters_and_counted() {
    // Codes 1 and 2 have glyph names no encoding resolves; code 3 maps to a control character.
    let font = Font {
        to_unicode: vec![(3, "\u{7}")],
        unnamed: true,
    };
    let page = build(
        &["BT /F1 10 Tf 1 0 0 1 20 300 Tm (A\\001\\002\\003B) Tj ET"],
        &font,
        0,
    )
    .extract_text(0, &CHARS)
    .unwrap();
    assert_eq!(page.text, "A\u{FFFD}\u{FFFD}\u{FFFD}B");
    let flags: Vec<u32> = page.clusters.unwrap().iter().map(|c| c.flags).collect();
    assert_eq!(flags, [0, 2, 2, 2, 0]);
    assert_eq!(page.stats.unmapped_glyphs, 3);
}

#[test]
fn invisible_ocr_layer_is_extracted_and_flagged() {
    let content =
        "BT /F1 10 Tf 1 0 0 1 20 300 Tm 3 Tr (scanned) Tj 0 Tr 1 0 0 1 20 200 Tm (typed) Tj ET";
    let page = build(&[content], &Font::default(), 0)
        .extract_text(0, &CHARS)
        .unwrap();
    let inv: Vec<_> = page
        .words
        .iter()
        .map(|w| (w.text.as_str(), w.invisible))
        .collect();
    assert_eq!(inv, [("scanned", true), ("typed", false)]);
    assert_eq!(
        (page.stats.invisible_clusters, page.stats.visible_clusters),
        (7, 5)
    );
    assert_eq!(
        page.styles[page.words[0].style_index as usize].render_mode,
        3
    );
    assert_eq!(page.clusters.unwrap()[0].flags & 1, 1);
}

#[test]
fn lines_words_blocks_follow_the_grouping_constants_and_sort_by_baseline_then_x() {
    // 10 pt: a 2.5 pt TJ gap (0.25 em) breaks a word, 1.5 pt (0.15 em) does not. Line 2 is 12 pt
    // below (1.2 em, same block), line 3 is 40 pt below (new block); "far" shares line 1's
    // baseline 100 pt to the right (gap > 5 em: its own line, and its own block).
    let content = "BT /F1 10 Tf 1 0 0 1 20 300 Tm [(ab) -250 (cd) -150 (ef)] TJ \
                   1 0 0 1 20 288 Tm (next) Tj 1 0 0 1 20 248 Tm (low) Tj 1 0 0 1 200 300 Tm (far) Tj ET";
    let page = build(&[content], &Font::default(), 0)
        .extract_text(0, &CHARS)
        .unwrap();
    let rows: Vec<_> = page
        .words
        .iter()
        .map(|w| (w.text.as_str(), w.block_index, w.line_index, w.word_index))
        .collect();
    // Text order is (y1, x0): line 1 left to right, "far" (same y1), then "next", then "low".
    assert_eq!(
        rows,
        [
            ("ab", 0, 0, 0),
            ("cdef", 0, 0, 1),
            ("far", 2, 0, 0),
            ("next", 0, 1, 0),
            ("low", 1, 0, 0)
        ]
    );
}

#[test]
fn rotated_pages_report_display_boxes_but_sort_in_unrotated_space() {
    // /Rotate 90: user (x, y) displays at (y, x). A is drawn first at user (50, 100) and B at
    // (150, 300). Unrotated, B is higher on the page (y 100 vs 300), so B sorts first, even
    // though in the displayed page A is above B.
    let content = "BT /F1 10 Tf 1 0 0 1 50 100 Tm (AAA) Tj 1 0 0 1 150 300 Tm (BBB) Tj ET";
    let page = build(&[content], &Font::default(), 90)
        .extract_text(0, &CHARS)
        .unwrap();
    assert_eq!(texts(&page), ["BBB", "AAA"]);
    let b = page.words[0].bounds;
    assert!(
        (b.x - 298.0).abs() < 3.0 && (b.y - 150.0).abs() < 1.0,
        "{b:?}"
    );
    // Display-space glyphs of upright user text read downward: width 15 along display y.
    assert!(close(b.height, 15.0) && b.width < 11.0, "{b:?}");
}
