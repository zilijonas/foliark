//! `Document::prepare_paragraph` (spec §6.1, §6.2, §6.4) on synthetic pages. Fixtures use
//! Helvetica at 10 pt with every glyph 5 pt wide, so line extents are exact.
mod common;

use common::{Font, build};
use foliark_engine::Document;
use foliark_model::{
    ErrorCode, ExtractOptions, FontMatchRule, FontResolution, FontRule, FontSource, FontStyle,
    ParagraphInfo, PdfError, Rect,
};
use foliark_text::Registry;
use pdf_writer::{Finish, Name, Pdf, Rect as PdfRect, Ref};

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn prepare(doc: &Document, rects: &[Rect]) -> Result<ParagraphInfo, PdfError> {
    doc.prepare_paragraph(0, rects, None, false, &Registry::new(), &[])
}

fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

/// Four per-line blocks of one paragraph (lines 1 and 2 wrap on their own, line 3 ends short) and
/// a lone word far below. Baselines (display) 100, 114, 128, 142 and 200.
/// Oracle (PyMuPDF 1.26.3 with HDDN's `_detect_line_step` and `_paragraph_layout`, run once on
/// this exact page): span origins 100/114/128/142, size 10.0, color 0, lineStep 14.0, union x0 20,
/// content right 145.0.
fn paragraph_page() -> Document {
    build(
        &["BT /F1 10 Tf 20 300 Td (The quick brown fox jumps) Tj ET\n\
             BT /F1 10 Tf 20 286 Td (over the lazy dogs again.) Tj ET\n\
             BT /F1 10 Tf 20 272 Td (Short end.) Tj ET\n\
             BT /F1 10 Tf 20 258 Td (Next line here) Tj ET\n\
             BT /F1 10 Tf 20 200 Td (wall) Tj ET"],
        &Font::default(),
        0,
    )
}

const ALL_FOUR_LINES: fn() -> Rect = || rect(10.0, 85.0, 150.0, 65.0);

#[test]
fn per_line_blocks_become_one_paragraph_matching_the_pymupdf_oracle() {
    let doc = paragraph_page();
    let p = prepare(&doc, &[ALL_FOUR_LINES()]).unwrap();
    near(p.font_size, 10.0);
    near(p.first_baseline, 100.0);
    near(p.line_step, 14.0);
    near(p.origin_x, 20.0);
    assert_eq!(p.color, [0.0, 0.0, 0.0]);
    assert!(!p.mixed_style && !p.bold && !p.italic);
    // One style, one segment over the whole text.
    let text = p.original_text.as_deref().unwrap();
    let [seg] = &p.segments[..] else {
        panic!("{:?}", p.segments);
    };
    assert_eq!(
        (seg.start, seg.end),
        (0, text.encode_utf16().count() as u32)
    );
    assert_eq!(
        (seg.font_size, seg.color, seg.bold),
        (p.font_size, p.color, p.bold)
    );
    // Content right edge 145 (oracle) and no word beside the paragraph.
    near(p.wrap_width, 125.0);
    // Soft wraps rejoin with a space; the break after the short line is hard.
    assert_eq!(
        p.original_text.as_deref(),
        Some("The quick brown fox jumps over the lazy dogs again. Short end.\nNext line here")
    );
}

#[test]
fn budget_stops_at_the_word_below_and_the_page_bottom() {
    let doc = paragraph_page();
    let words = doc
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .words;
    let wall = words.iter().find(|w| w.text == "wall").unwrap().bounds;
    // Four lines, then the lone word 58 pt lower: floor((192 + 0.1 - 100 - 2) / 14) + 1 = 7.
    let p = prepare(&doc, &[ALL_FOUR_LINES()]).unwrap();
    near(p.max_height, wall.y - p.top);
    near(wall.y, 192.0);
    assert_eq!(p.max_lines, 7);

    // Only line 1 selected: lines 2..4 are not selected, so they are the content below and the
    // paragraph may not grow into them. One line, 14 pt of height (line 2's top is 106).
    let one = prepare(&doc, &[rect(10.0, 85.0, 150.0, 14.0)]).unwrap();
    near(one.max_height, 14.0);
    assert_eq!(one.max_lines, 1);

    // The lone word has nothing below: the bottom margin mirrors the top one (92 -> 308), so
    // floor((308 + 0.1 - 200 - 2) / 12) + 1 = 9. Its wrap runs to the page's content right (145).
    let last = prepare(&doc, &[rect(10.0, 185.0, 60.0, 20.0)]).unwrap();
    near(last.line_step, 12.0);
    near(last.wrap_width, 125.0);
    assert_eq!(last.max_lines, 9);
}

#[test]
fn columns_do_not_borrow_each_others_space() {
    let doc = build(
        &["BT /F1 10 Tf 20 300 Td (left column a) Tj ET\n\
           BT /F1 10 Tf 20 286 Td (left column b) Tj ET\n\
           BT /F1 10 Tf 170 300 Td (right column a long) Tj ET\n\
           BT /F1 10 Tf 170 286 Td (right column b) Tj ET\n\
           BT /F1 10 Tf 170 250 Td (right below) Tj ET"],
        &Font::default(),
        0,
    );
    let words = doc
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .words;
    let below = words.iter().find(|w| w.text == "below").unwrap().bounds.y;
    let left = prepare(&doc, &[rect(10.0, 85.0, 140.0, 40.0)]).unwrap();
    // The right column starts beside the paragraph: wrap ends at the paragraph's own right edge
    // (13 chars x 5 pt from x = 20) and the right column's word below does not limit it.
    near(left.wrap_width, 65.0);
    assert_eq!(
        left.original_text.as_deref(),
        Some("left column a left column b")
    );
    near(left.max_height, 400.0 - 92.0 - left.top);

    let right = prepare(&doc, &[rect(160.0, 85.0, 140.0, 40.0)]).unwrap();
    near(right.origin_x, 170.0);
    // Nothing beside it: wrap to the page's content right edge (170 + 19 * 5).
    near(right.wrap_width, 95.0);
    near(right.max_height, below - right.top);
    assert_eq!(
        right.original_text.as_deref(),
        Some("right column a long right column b")
    );
}

#[test]
fn overprinted_faux_bold_reads_once_and_real_double_letters_stay() {
    let doc = build(
        &["BT /F1 10 Tf 20 300 Td (Hello Invoice) Tj ET\n\
           BT /F1 10 Tf 20.6 300 Td (Hello Invoice) Tj ET"],
        &Font::default(),
        0,
    );
    let p = prepare(&doc, &[rect(0.0, 80.0, 300.0, 30.0)]).unwrap();
    assert_eq!(p.original_text.as_deref(), Some("Hello Invoice"));
}

#[test]
fn unmapped_clusters_give_no_original_text() {
    let doc = build(
        &["BT /F1 10 Tf 20 300 Td (AB) Tj ET"],
        &Font {
            unnamed: true,
            ..Font::default()
        },
        0,
    );
    // Without a ToUnicode CMap the unnamed glyphs have no Unicode; plain letters still map.
    let p = prepare(&doc, &[rect(0.0, 80.0, 300.0, 30.0)]).unwrap();
    assert_eq!(p.original_text.as_deref(), Some("AB"));
    let doc = build(
        &["BT /F1 10 Tf 20 300 Td <0102> Tj ET"],
        &Font {
            unnamed: true,
            ..Font::default()
        },
        0,
    );
    let p = prepare(&doc, &[rect(0.0, 80.0, 300.0, 30.0)]).unwrap();
    assert_eq!(p.original_text, None);
}

#[test]
fn style_is_the_first_cluster_and_mixing_is_flagged() {
    let doc = build(
        &["BT /F1 10 Tf 20 300 Td (Plain ) Tj 1 0 0 rg (red) Tj ET\n\
           BT /F1 10 Tf 0 1 0 rg 20 200 Td (green) Tj /F1 14 Tf (big) Tj ET"],
        &Font::default(),
        0,
    );
    let p = prepare(&doc, &[rect(0.0, 80.0, 300.0, 30.0)]).unwrap();
    assert!(p.mixed_style);
    assert_eq!(p.color, [0.0, 0.0, 0.0]);
    let q = prepare(&doc, &[rect(0.0, 180.0, 300.0, 30.0)]).unwrap();
    assert!(q.mixed_style);
    assert_eq!(q.color, [0.0, 1.0, 0.0]);
    near(q.font_size, 10.0);
    // Selecting only the uniform "Plain" is not mixed.
    assert!(
        !prepare(&doc, &[rect(0.0, 80.0, 45.0, 30.0)])
            .unwrap()
            .mixed_style
    );
}

#[test]
fn rotated_invisible_and_rtl_text_is_refused_with_the_spec_codes() {
    let code = |r: Result<ParagraphInfo, PdfError>| r.unwrap_err().code;
    let everywhere = [rect(0.0, 0.0, 300.0, 400.0)];

    let slanted = build(
        &["BT /F1 10 Tf 0.866 0.5 -0.5 0.866 50 200 Tm (slanted) Tj ET"],
        &Font::default(),
        0,
    );
    assert_eq!(
        code(prepare(&slanted, &everywhere)),
        ErrorCode::EditRotatedText
    );
    // Upright but skewed is refused too.
    let skew = build(
        &["BT /F1 10 Tf 1 0 0.5 1 50 200 Tm (skewed) Tj ET"],
        &Font::default(),
        0,
    );
    assert_eq!(
        code(prepare(&skew, &everywhere)),
        ErrorCode::EditRotatedText
    );

    // The same text turned a quarter and shown on a /Rotate 90 page reads upright: accepted.
    let turned = "BT /F1 10 Tf 0 1 -1 0 100 50 Tm (turned page) Tj ET";
    assert_eq!(
        code(prepare(&build(&[turned], &Font::default(), 0), &everywhere)),
        ErrorCode::EditRotatedText
    );
    let p = prepare(&build(&[turned], &Font::default(), 90), &everywhere).unwrap();
    near(p.first_baseline, 100.0);
    near(p.origin_x, 50.0);
    assert_eq!(p.original_text.as_deref(), Some("turned page"));

    // OCR layer (render mode 3): nothing visible to edit. So is an empty area.
    let ocr = build(
        &["BT /F1 10 Tf 3 Tr 20 300 Td (scanned text) Tj ET"],
        &Font::default(),
        0,
    );
    assert_eq!(code(prepare(&ocr, &everywhere)), ErrorCode::EditNoStyle);
    assert_eq!(
        code(prepare(
            &paragraph_page(),
            &[rect(250.0, 300.0, 10.0, 10.0)]
        )),
        ErrorCode::EditNoStyle
    );

    // Hebrew alef drawn through ToUnicode.
    let rtl = build(
        &["BT /F1 10 Tf 20 300 Td (AAA) Tj ET"],
        &Font {
            to_unicode: vec![(b'A', "\u{5D0}")],
            ..Font::default()
        },
        0,
    );
    let e = prepare(&rtl, &everywhere).unwrap_err();
    assert_eq!(e.code, ErrorCode::EditUnsupportedScript);
    assert_eq!(
        e.details.unwrap().chars.as_deref(),
        Some("\u{5D0}\u{5D0}\u{5D0}")
    );
}

/// A page showing "aaa" in a Type3 font named `base_font`. Type3 is never reused as-is, so the
/// registered rungs of the ladder decide (and the name still reaches the name-match rung).
fn type3_page(base_font: &str) -> Document {
    let mut pdf = Pdf::new();
    let [cat, tree, page, content, font, glyph] = [1, 2, 3, 4, 5, 6].map(Ref::new);
    pdf.catalog(cat).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    let mut p = pdf.page(page);
    p.parent(tree)
        .media_box(PdfRect::new(0.0, 0.0, 300.0, 400.0))
        .contents(content);
    p.resources().fonts().pair(Name(b"F1"), font);
    p.finish();
    pdf.stream(content, b"BT /F1 10 Tf 20 300 Td (aaa) Tj ET");
    pdf.stream(glyph, b"500 0 d0 0 0 400 700 re f");
    let mut f = pdf.indirect(font).dict();
    f.pair(Name(b"Type"), Name(b"Font"));
    f.pair(Name(b"Subtype"), Name(b"Type3"));
    f.pair(Name(b"BaseFont"), Name(base_font.as_bytes()));
    f.insert(Name(b"FontBBox"))
        .array()
        .items([0.0, 0.0, 1000.0, 1000.0]);
    f.insert(Name(b"FontMatrix"))
        .array()
        .items([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]);
    f.insert(Name(b"CharProcs")).dict().pair(Name(b"a"), glyph);
    let mut enc = f.insert(Name(b"Encoding")).dict();
    enc.insert(Name(b"Differences"))
        .array()
        .item(97)
        .item(Name(b"a"));
    enc.finish();
    f.pair(Name(b"FirstChar"), 97);
    f.pair(Name(b"LastChar"), 97);
    f.insert(Name(b"Widths")).array().items([500.0]);
    f.finish();
    Document::open(pdf.finish(), None).unwrap()
}

fn liberation(file: &str, weight: u16) -> FontSource {
    FontSource {
        bytes: std::fs::read(format!(
            "{}/../foliark-text/tests/data/{file}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
        family: "Liberation Sans".into(),
        weight,
        style: FontStyle::Normal,
    }
}

#[test]
fn font_resolution_follows_the_ladder_and_reports_its_rule() {
    let mut reg = Registry::new();
    let regular = reg
        .register(liberation("LiberationSans-Regular.ttf", 400))
        .unwrap();
    let bold = reg
        .register(liberation("LiberationSans-Bold.ttf", 700))
        .unwrap();
    let all = [rect(0.0, 0.0, 300.0, 400.0)];
    let resolve = |doc: &Document, pick: Option<&str>, rules: &[FontMatchRule]| {
        doc.prepare_paragraph(0, &all, pick, true, &reg, rules)
            .unwrap()
    };

    // Name match: "arial" -> Liberation Sans, bold from the name; editor face = registered bytes.
    let arial = type3_page("ABCDEF+Arial-BoldMT");
    let p = resolve(&arial, None, &[]);
    assert_eq!(
        p.font,
        FontResolution::Registered {
            font_id: bold.id.clone(),
            family: "Liberation Sans".into(),
            rule: FontRule::NameMatch
        }
    );
    assert!(p.bold);
    assert_eq!(
        p.editor_face.unwrap().bytes,
        liberation("LiberationSans-Bold.ttf", 700).bytes
    );

    // Unknown name: flags rung (plain sans).
    let odd = type3_page("Zzyzx");
    let p = resolve(&odd, None, &[]);
    assert_eq!(
        p.font,
        FontResolution::Registered {
            font_id: regular.id.clone(),
            family: "Liberation Sans".into(),
            rule: FontRule::Flags
        }
    );
    // A caller rule wins over the default table; a caller pick wins over everything.
    let rule = [FontMatchRule {
        pattern: "zzy".into(),
        family: "Liberation Sans".into(),
    }];
    assert!(matches!(
        resolve(&odd, None, &rule).font,
        FontResolution::Registered {
            rule: FontRule::NameMatch,
            ..
        }
    ));
    let p = resolve(&arial, Some(&regular.id), &[]);
    assert!(
        matches!(&p.font, FontResolution::Registered { font_id, rule: FontRule::Caller, .. } if *font_id == regular.id)
    );

    // No font at all can draw it.
    let e = odd
        .prepare_paragraph(0, &all, None, false, &Registry::new(), &[])
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::EditNoFont);
}

#[test]
fn a_reusable_source_font_is_reported_as_embedded_without_an_editor_face_it_cannot_make() {
    let doc = paragraph_page();
    let p = doc
        .prepare_paragraph(0, &[ALL_FOUR_LINES()], None, true, &Registry::new(), &[])
        .unwrap();
    // Helvetica without a font program: reused (the viewer draws it), but there are no bytes.
    assert!(matches!(
        &p.font,
        FontResolution::Embedded { name, covers_ascii: false, .. } if name == "Helvetica"
    ));
    assert!(p.editor_face.is_none());
}
