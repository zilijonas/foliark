//! A bold heading line directly above its regular body, as Word writes an article, offered as one
//! editable block (ADR 0012; NEW-106). Editing one part leaves the other as it is on the page and
//! lays the changed part out in its own style, inside its own right edge. The fixture's faces are
//! TrueType WinAnsi subsets named as Word names Arial: bold 1.2 em, regular 1 em, size 5. The
//! export is judged by the objects and fidelity oracles.
mod common;
use common::{Font, build, fidelity_oracle, fixture, objects_oracle, run};
use foliark_engine::Engine;
use foliark_model::{
    ChangeResult, ChangeSet, ErrorCode, ExportMode, ExportPolicy, ExtractOptions, FontSource,
    FontStyle, Limits, Rect, RunSource, TextEdit, TextPage,
};
use foliark_text::Registry;

const FILE: &str = "generated/word-bold-heading-body.pdf";
/// The block HDDN offers: the heading row (baseline 100) and the body's three rows (108, 115, 122).
const BLOCK: Rect = rect(15.0, 94.0, 165.0, 31.0);
/// Display boxes `(page, x0, y0, x1, y1)`; 170 is the page's right edge (the body's rows end there).
const BODY: (u32, f64, f64, f64, f64) = (0, 20.0, 104.0, 170.0, 123.0);
const HEADING: (u32, f64, f64, f64, f64) = (0, 20.0, 96.0, 170.0, 101.0);
const HEAD: &str = "1 ARTICLE. SCOPE";
const BODY_TEXT: &str =
    "alpha bravo charlie delta echo foxtrot golf hotels india lima mikes oscar papa quebec romeos";

const fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn open() -> (Engine, String) {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(fixture(FILE), None, false).unwrap();
    (e, doc)
}

fn commit(e: &mut Engine, doc: &str, text: &str) -> ChangeResult {
    let t = e.apply_changes(
        doc,
        ChangeSet {
            base_revision: e.revision(doc).unwrap(),
            text_edits: Some(vec![TextEdit {
                id: "e".into(),
                page_index: 0,
                rects: vec![BLOCK],
                text: text.into(),
                font: None,
            }]),
            redactions: None,
        },
    );
    run(e, t).unwrap()
}

fn page(e: &mut Engine, doc: &str) -> TextPage {
    let t = e.extract_text(doc, 0, ExtractOptions::default());
    run(e, t).unwrap()
}

/// `(text, x0, y0, x1, font name, bold)` of every word whose box centre lies in `b`.
fn words(p: &TextPage, b: (u32, f64, f64, f64, f64)) -> Vec<(String, [f64; 3], String, bool)> {
    p.words
        .iter()
        .filter(|w| {
            let cy = w.bounds.y + w.bounds.height / 2.0;
            (b.2..=b.4).contains(&cy)
        })
        .map(|w| {
            let s = &p.styles[w.style_index as usize];
            let r = |v: f64| (v * 100.0).round() / 100.0;
            let x1 = w.bounds.x + w.bounds.width;
            (
                w.text.clone(),
                [r(w.bounds.x), r(w.bounds.y), r(x1)],
                s.font_name.clone(),
                s.flags.bold,
            )
        })
        .collect()
}

fn export(e: &mut Engine, doc: &str) -> Vec<u8> {
    let t = e.prepare_export(doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(e, t).unwrap();
    e.take_export(doc, &plan.plan_id).unwrap()
}

#[test]
fn a_mixed_block_reports_one_segment_per_style_and_they_rejoin_to_its_text() {
    let (mut e, doc) = open();
    let info = e
        .prepare_paragraph(&doc, 0, vec![BLOCK], None, true)
        .unwrap();
    let text: Vec<u16> = info
        .original_text
        .as_deref()
        .unwrap()
        .encode_utf16()
        .collect();
    let [head, body] = &info.segments[..] else {
        panic!("{:?}", info.segments);
    };
    // Contiguous from 0 to the end: the heading's LF ends the heading.
    assert_eq!(
        (head.start, head.end, body.end),
        (0, HEAD.len() as u32 + 1, text.len() as u32)
    );
    assert_eq!(body.start, head.end);
    let rejoined: Vec<u16> = info
        .segments
        .iter()
        .flat_map(|s| text[s.start as usize..s.end as usize].to_vec())
        .collect();
    assert_eq!(rejoined, text);
    assert!(head.bold && !body.bold, "{head:?} {body:?}");
    // Each segment is the page's own font, with the face an editor draws it in; the two differ.
    let name = |s: &foliark_model::ParagraphSegment| match &s.font {
        foliark_model::FontResolution::Embedded { name, .. } => name.clone(),
        other => panic!("{other:?}"),
    };
    assert!(name(head).contains("Bold") && !name(body).contains("Bold"));
    let faces: Vec<_> = info
        .segments
        .iter()
        .map(|s| s.editor_face.as_ref().map(|f| f.bytes.len()))
        .collect();
    assert!(faces.iter().all(|f| f.is_some_and(|n| n > 0)), "{faces:?}");
    assert_ne!(head.editor_face, body.editor_face);
}

#[test]
fn editing_the_body_keeps_the_heading_and_both_styles_inside_the_right_edge() {
    let (mut e, doc) = open();
    let info = e
        .prepare_paragraph(&doc, 0, vec![BLOCK], None, false)
        .unwrap();
    assert_eq!(
        info.original_text.as_deref(),
        Some(format!("{HEAD}\n{BODY_TEXT}").as_str())
    );
    // The block's style is the body's, which most of it is drawn in; the heading makes it mixed.
    assert!(!info.bold && info.mixed_style, "{info:?}");
    let before = page(&mut e, &doc);

    let text = format!("{HEAD}\n{}", BODY_TEXT.replace("charlie", "charles"));
    let p = e
        .layout_edit(&doc, 0, vec![BLOCK], &text, None, true)
        .unwrap();
    let lines: Vec<(&str, f64)> = p
        .lines
        .iter()
        .map(|l| (l.text.as_str(), l.baseline))
        .collect();
    assert_eq!(
        lines,
        [
            ("alpha bravo charles delta echo", 108.0),
            ("foxtrot golf hotels india lima", 115.0),
            ("mikes oscar papa quebec romeos", 122.0)
        ],
        "{p:?}"
    );
    assert!(p.fits && !p.approximate, "{p:?}");
    assert!(p.runs.iter().all(|r| r.source == RunSource::Embedded));
    assert!(
        p.lines.iter().all(|l| l.x + l.width <= 170.0 + 1e-6),
        "{p:?}"
    );
    // Glyph offsets index the whole text the caller sent: the body starts after the heading's LF.
    let first = &p.glyphs.as_ref().unwrap()[0];
    assert_eq!((first.start, first.x), (HEAD.len() as u32 + 1, 20.0));

    let c = commit(&mut e, &doc, &text);
    assert!(c.accepted, "{:?}", c.rejected);
    assert_eq!(c.layouts[0].fingerprint, p.fingerprint);
    assert_eq!(c.layouts[0].lines, 3);

    let after = page(&mut e, &doc);
    // The heading is the page's own text, where it was, still bold.
    let head = words(&before, HEADING);
    assert_eq!(words(&after, HEADING), head);
    assert!(
        head.iter().all(|w| w.2 == "Arial-BoldMT" && w.3),
        "{head:?}"
    );
    // The body is regular, in the body's font, and no word reaches past the body's right edge.
    let body = words(&after, BODY);
    assert_eq!(body.len(), 15, "{body:?}");
    assert!(
        body.iter()
            .all(|w| w.2 == "ArialMT" && !w.3 && w.1[2] <= 170.0),
        "{body:?}"
    );

    let input = fixture(FILE);
    let out = export(&mut e, &doc);
    let (ok, log) = objects_oracle("paragraph-styles", "input.pdf", &input, &["charlie"]);
    assert!(
        !ok && log.contains("charlie"),
        "the oracle must see the word in the input: {log}"
    );
    let (ok, log) = objects_oracle("paragraph-styles", "body.pdf", &out, &["charlie"]);
    assert!(ok, "objects.py: {log}");
    // Nothing outside the body's box changed: the heading, the KEEP line and the margin right of
    // the body look and read as before.
    let (ok, log) = fidelity_oracle(
        "paragraph-styles",
        "body",
        &input,
        &out,
        &[BODY],
        &[HEAD, "KEEP_STYLE_LINE"],
    );
    assert!(ok, "fidelity.py: {log}");
}

#[test]
fn editing_the_heading_keeps_the_body() {
    let (mut e, doc) = open();
    let before = page(&mut e, &doc);
    let text = format!("1 ARTICLE. SCOPES\n{BODY_TEXT}");
    let p = e
        .layout_edit(&doc, 0, vec![BLOCK], &text, None, false)
        .unwrap();
    assert_eq!(p.lines.len(), 1, "{p:?}");
    assert!(p.fits && p.max_lines == 1, "{p:?}");
    let c = commit(&mut e, &doc, &text);
    assert!(c.accepted, "{:?}", c.rejected);
    assert_eq!(c.layouts[0].fingerprint, p.fingerprint);

    let after = page(&mut e, &doc);
    assert_eq!(words(&after, BODY), words(&before, BODY));
    let head = words(&after, HEADING);
    assert_eq!(head.last().unwrap().0, "SCOPES");
    assert!(
        head.iter().all(|w| w.2 == "Arial-BoldMT" && w.3),
        "{head:?}"
    );

    let out = export(&mut e, &doc);
    let (ok, log) = fidelity_oracle(
        "paragraph-styles",
        "heading",
        &fixture(FILE),
        &out,
        &[HEADING],
        &[BODY_TEXT.split(' ').next().unwrap(), "KEEP_STYLE_LINE"],
    );
    assert!(ok, "fidelity.py: {log}");
}

#[test]
fn unchanged_text_changes_nothing_and_longer_text_still_does_not_fit() {
    let (mut e, doc) = open();
    let before = page(&mut e, &doc);
    let original = format!("{HEAD}\n{BODY_TEXT}");
    let same = e
        .layout_edit(&doc, 0, vec![BLOCK], &original, None, false)
        .unwrap();
    assert!(same.fits && same.lines.is_empty(), "{same:?}");
    let c = commit(&mut e, &doc, &original);
    assert!(c.accepted && c.layouts[0].lines == 0, "{c:?}");
    assert_eq!(page(&mut e, &doc).words, before.words);

    // Four lines fit above the KEEP line; this body needs five.
    let text = format!("{HEAD}\n{BODY_TEXT} sierra tango uniform victor hotel india lima");
    let p = e
        .layout_edit(&doc, 0, vec![BLOCK], &text, None, false)
        .unwrap();
    assert!(!p.fits, "{p:?}");
    assert_eq!((p.needed_lines, p.max_lines), (5, 4));
    let c = commit(&mut e, &doc, &text);
    assert_eq!(c.rejected[0].code, ErrorCode::EditNoFit);
}

#[test]
fn a_wider_caller_font_does_not_widen_the_box() {
    // The page draws "mmmm mmmm" with Helvetica's own /Widths (0.5 em): 45 pt. The caller's bold
    // face draws `m` 0.89 em wide; measuring the box with it would move the right edge (ADR 0012).
    let mut reg = Registry::new();
    let bold = reg
        .register(FontSource {
            bytes: std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../foliark-text/tests/data/LiberationSans-Bold.ttf"
            ))
            .unwrap(),
            family: "Liberation Sans".into(),
            weight: 700,
            style: FontStyle::Normal,
        })
        .unwrap();
    let doc = build(
        &["BT /F1 10 Tf 20 300 Td (mmmm mmmm) Tj ET"],
        &Font::default(),
        0,
    );
    let all = [rect(0.0, 0.0, 300.0, 400.0)];
    let plain = doc
        .prepare_paragraph(0, &all, None, false, &reg, &[])
        .unwrap();
    let wide = doc
        .prepare_paragraph(0, &all, Some(&bold.id), false, &reg, &[])
        .unwrap();
    assert_eq!(
        (plain.wrap_width, wide.wrap_width),
        (45.0, 45.0),
        "{wide:?}"
    );
}
