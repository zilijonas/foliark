//! Search (spec §3.5): reversible normalization, cross-line matches, exact quads, limits.
mod common;
use common::{Font, build, close, width};
use foliark_model::SearchOptions;

fn opts() -> SearchOptions {
    SearchOptions::default()
}

fn one(content: &str, font: &Font) -> foliark_engine::Document {
    build(&[content], font, 0)
}

#[test]
fn case_and_diacritic_options_with_decomposed_marks() {
    // "žaba", "zaba", "ZABA" and a decomposed "a" + ogonek ("ą") on separate words.
    let font = Font {
        to_unicode: vec![(1, "\u{17E}"), (2, "\u{328}")],
        unnamed: false,
    };
    let doc = one(
        "BT /F1 10 Tf 1 0 0 1 20 300 Tm (\\001aba zaba ZABA ) Tj [(a) 400 (\\002)] TJ ET",
        &font,
    );
    let hits = |o: SearchOptions, q: &str| {
        doc.search(q, &o)
            .unwrap()
            .iter()
            .map(|h| h.range.text.clone())
            .collect::<Vec<_>>()
    };
    // Default: case-insensitive, diacritic-sensitive.
    assert_eq!(hits(opts(), "zaba"), ["zaba", "ZABA"]);
    assert_eq!(
        hits(
            SearchOptions {
                case_sensitive: true,
                ..opts()
            },
            "zaba"
        ),
        ["zaba"]
    );
    // Diacritic-insensitive: "ž" matches "z" both ways, and the separately drawn ogonek is
    // dropped from matching, but a hit ending on its base covers it.
    let off = SearchOptions {
        diacritic_sensitive: false,
        ..opts()
    };
    assert_eq!(hits(off.clone(), "zaba"), ["\u{17E}aba", "zaba", "ZABA"]);
    assert_eq!(hits(off.clone(), "ž"), ["\u{17E}", "z", "Z"]);
    assert_eq!(hits(off, "zaba a"), ["ZABA a\u{328}"]);
}

#[test]
fn ligatures_match_both_ways_and_a_partial_cluster_hit_is_flagged() {
    // Glyph 1 is U+FB01, glyph 2 is the two code points "fi" in one cluster.
    let font = Font {
        to_unicode: vec![(1, "\u{FB01}"), (2, "fi")],
        unnamed: false,
    };
    let doc = one(
        "BT /F1 10 Tf 1 0 0 1 20 300 Tm (\\001sh) Tj ( ) Tj (\\002sh) Tj ( ) Tj (fish) Tj ET",
        &font,
    );
    let r = doc.search("fish", &opts()).unwrap();
    assert_eq!(
        r.iter().map(|h| h.range.text.as_str()).collect::<Vec<_>>(),
        ["\u{FB01}sh", "fish", "fish"]
    );
    assert!(r.iter().all(|h| !h.range.partial_cluster));
    assert_eq!(doc.search("\u{FB01}sh", &opts()).unwrap().len(), 3);
    // "f" matches half of the fi cluster: the hit snaps to the whole cluster and says so.
    let f = doc.search("f", &opts()).unwrap();
    let in_lig = &f[0].range;
    assert_eq!(
        (in_lig.start, in_lig.end, in_lig.partial_cluster),
        (0, 1, true)
    );
    assert!(close(width(&in_lig.quads[0]), 5.0));
    let in_pair = &f[1].range;
    assert!(in_pair.partial_cluster && in_pair.text == "fi");
}

#[test]
fn matches_cross_lines_and_spans_with_one_quad_per_line() {
    // "dog" ends line 1 and "jumps" starts line 2; "hel" + "lo" are two Tj operators with no gap.
    let content = "BT /F1 10 Tf 1 0 0 1 20 300 Tm (the dog) Tj 1 0 0 1 20 288 Tm (jumps over) Tj \\
                   1 0 0 1 20 200 Tm (hel) Tj (lo) Tj ET";
    let doc = one(content, &Font::default());
    let hit = &doc.search("dog   jumps", &opts()).unwrap()[0];
    assert_eq!(
        (hit.range.start, hit.range.end, hit.range.text.as_str()),
        (4, 13, "dog jumps")
    );
    assert_eq!(hit.range.quads.len(), 2);
    assert!(close(hit.range.quads[0].0[0].x, 40.0) && close(hit.range.quads[1].0[1].x, 45.0));
    let span = &doc.search("hello", &opts()).unwrap()[0];
    assert_eq!(
        (span.range.text.as_str(), span.range.quads.len()),
        ("hello", 1)
    );
}

#[test]
fn repeated_hits_do_not_overlap_and_carry_exact_quads_and_offsets() {
    let doc = one(
        "BT /F1 10 Tf 1 0 0 1 20 300 Tm (ab ab aab) Tj ET",
        &Font::default(),
    );
    let hits = doc.search("ab", &opts()).unwrap();
    let got: Vec<_> = hits
        .iter()
        .map(|h| (h.range.start, h.range.end, h.range.quads[0].0[0].x))
        .collect();
    assert_eq!(got, [(0, 2, 20.0), (3, 5, 35.0), (7, 9, 55.0)]);
    assert!(
        hits.iter()
            .all(|h| close(width(&h.range.quads[0]), 10.0) && h.page_index == 0)
    );
    // Overlapping candidates: "aa" in "aaa" is one hit, not two.
    let aaa = one(
        "BT /F1 10 Tf 1 0 0 1 20 300 Tm (aaa) Tj ET",
        &Font::default(),
    );
    assert_eq!(aaa.search("aa", &opts()).unwrap().len(), 1);
}

#[test]
fn whole_word_quotes_dashes_and_limits() {
    let doc = one(
        "BT /F1 10 Tf 1 0 0 1 20 300 Tm (concat cat category cat.) Tj ET",
        &Font::default(),
    );
    let whole = SearchOptions {
        whole_word: true,
        ..opts()
    };
    let at: Vec<u32> = doc
        .search("cat", &whole)
        .unwrap()
        .iter()
        .map(|h| h.range.start)
        .collect();
    assert_eq!(at, [7, 20]);
    assert_eq!(
        doc.search(
            "cat",
            &SearchOptions {
                max_hits: 2,
                ..opts()
            }
        )
        .unwrap()
        .len(),
        2
    );
    // Curly quote and en dash in the text match straight ones in the query.
    let font = Font {
        to_unicode: vec![(1, "\u{2019}"), (2, "\u{2013}")],
        unnamed: false,
    };
    let punct = one(
        "BT /F1 10 Tf 1 0 0 1 20 300 Tm (don\\001t a\\002b) Tj ET",
        &font,
    );
    assert_eq!(punct.search("don't a-b", &opts()).unwrap().len(), 1);
    assert!(doc.search("", &opts()).unwrap().is_empty());
}

#[test]
fn pages_filter_max_hits_and_no_match_across_pages() {
    let doc = build(
        &[
            "BT /F1 10 Tf 1 0 0 1 20 300 Tm (alpha foo) Tj ET",
            "BT /F1 10 Tf 1 0 0 1 20 300 Tm (bar foo) Tj ET",
        ],
        &Font::default(),
        0,
    );
    let pages = |o: SearchOptions| {
        doc.search("foo", &o)
            .unwrap()
            .iter()
            .map(|h| h.page_index)
            .collect::<Vec<_>>()
    };
    assert_eq!(pages(opts()), [0, 1]);
    assert_eq!(
        pages(SearchOptions {
            pages: Some(vec![1]),
            ..opts()
        }),
        [1]
    );
    assert_eq!(
        pages(SearchOptions {
            max_hits: 1,
            ..opts()
        }),
        [0]
    );
    assert!(
        doc.search("foo bar", &opts()).unwrap().is_empty(),
        "the end of page 0 and the start of page 1 are not adjacent text"
    );
    assert!(
        doc.search(
            "foo",
            &SearchOptions {
                pages: Some(vec![5]),
                ..opts()
            }
        )
        .is_err()
    );
}
