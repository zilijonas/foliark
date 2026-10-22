//! Edit previews (ADR 0004; NEW-52): `layout_edit` returns exactly the layout `apply_changes`
//! commits (same fingerprint, lines, runs), refuses as it would, and changes nothing.
mod common;
use common::{Font, pdf_bytes, run};
use foliark_engine::Engine;
use foliark_model::{
    ChangeSet, ErrorCode, ExtractOptions, FontSource, FontStyle, Limits, Rect, TextEdit,
};

const PAGE: &str = "BT /F1 10 Tf 20 300 Td (The quick brown fox jumps) Tj ET\n\
    BT /F1 10 Tf 20 286 Td (over the lazy dogs again.) Tj ET\n\
    BT /F1 10 Tf 20 200 Td (Keep this neighbour line) Tj ET";
const A: Rect = Rect {
    x: 10.0,
    y: 85.0,
    width: 150.0,
    height: 37.0,
};

fn engine(fonts: bool) -> (Engine, String) {
    let mut e = Engine::new(Limits::default());
    if fonts {
        e.register_fonts(vec![FontSource {
            bytes: std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../foliark-text/tests/data/LiberationSans-Regular.ttf"
            ))
            .unwrap(),
            family: "Liberation Sans".into(),
            weight: 400,
            style: FontStyle::Normal,
        }])
        .unwrap();
    }
    let (doc, _) = e
        .open(pdf_bytes(&[PAGE], &Font::default(), 0), None, false)
        .unwrap();
    (e, doc)
}

fn commit(e: &mut Engine, doc: &str, text: &str) -> foliark_model::ChangeResult {
    let t = e.apply_changes(
        doc,
        ChangeSet {
            base_revision: e.revision(doc).unwrap(),
            text_edits: Some(vec![TextEdit {
                id: "e".into(),
                page_index: 0,
                rects: vec![A],
                text: text.into(),
                font: None,
            }]),
            redactions: None,
        },
    );
    run(e, t).unwrap()
}

fn page_text(e: &mut Engine, doc: &str) -> String {
    let t = e.extract_text(doc, 0, ExtractOptions::default());
    run(e, t).unwrap().text
}

#[test]
fn a_preview_is_the_layout_the_commit_makes() {
    // Source-font only, composed Lithuanian (registered fallback), a hard break, a decomposed
    // letter (NFC on input), and deletion down to nothing.
    let texts = [
        "The",
        "The quick brown fox jumps over",
        "ąčęėįšųūž testing\nŽaliasis rašalas",
        "Z\u{30c}aliasis ras\u{30c}alas",
        "The quick\n\nover",
        "",
    ];
    for text in texts {
        let (mut e, doc) = engine(true);
        let before = page_text(&mut e, &doc);
        let p = e.layout_edit(&doc, 0, vec![A], text, None, true).unwrap();
        assert_eq!(
            (e.revision(&doc).unwrap(), page_text(&mut e, &doc)),
            (0, before)
        );
        let r = commit(&mut e, &doc, text);
        assert!(r.accepted, "{text:?} {:?}", r.rejected);
        let l = &r.layouts[0];
        assert_eq!(p.fingerprint, l.fingerprint, "{text:?}");
        assert!(p.fits, "{text:?}");
        assert_eq!(
            (p.lines.len() as u32, p.approximate, &p.runs),
            (l.lines, l.approximate, &l.runs),
            "{text:?}"
        );
        assert_eq!((p.revision, p.needed_lines), (0, l.lines));
        // The boxes tile each line from its origin and sum to its width.
        for (n, line) in p.lines.iter().enumerate() {
            let g: Vec<_> = p
                .glyphs
                .iter()
                .flatten()
                .filter(|g| g.line == n as u32)
                .collect();
            assert!(
                g.windows(2)
                    .all(|w| (w[0].x + w[0].width - w[1].x).abs() < 1e-9),
                "{text:?}"
            );
            assert!((g.iter().map(|g| g.width).sum::<f64>() - line.width).abs() < 1e-9);
            assert!(g.first().is_none_or(|g| g.x == line.x));
        }
        assert_eq!(p.lines.is_empty(), text.is_empty());
    }
}

#[test]
fn overflow_previews_as_not_fitting_and_the_commit_refuses_with_the_same_counts() {
    let (mut e, doc) = engine(true);
    let long = "quick brown fox ".repeat(30);
    let long = long.as_str();
    let p = e.layout_edit(&doc, 0, vec![A], long, None, false).unwrap();
    assert!(
        !p.fits && p.needed_lines > p.max_lines && p.glyphs.is_none(),
        "{p:?}"
    );
    assert_eq!(p.lines.len() as u32, p.needed_lines);
    let r = commit(&mut e, &doc, long);
    let d = r.rejected[0].details.as_ref().unwrap();
    assert!(!r.accepted && r.rejected[0].code == ErrorCode::EditNoFit);
    assert_eq!(
        (d.needed_lines, d.max_lines),
        (Some(p.needed_lines), Some(p.max_lines))
    );
    // One word wider than the line fits no number of lines.
    let w = e
        .layout_edit(&doc, 0, vec![A], &"W".repeat(60), None, false)
        .unwrap();
    assert!(!w.fits && w.needed_lines > w.max_lines);
}

#[test]
fn a_preview_refuses_as_the_commit_does() {
    let (e, doc) = engine(false);
    // The source font lacks these and nothing is registered.
    let err = e
        .layout_edit(&doc, 0, vec![A], "ąč", None, false)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::EditNoFont);
    let nowhere = Rect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };
    let err = e
        .layout_edit(&doc, 0, vec![nowhere], "x", None, false)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::EditNoStyle);
    let err = e
        .layout_edit(&doc, 0, vec![A], "a\u{202e}b", None, false)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::EditUnsupportedScript);
    assert_eq!(
        e.layout_edit("nope", 0, vec![A], "x", None, false)
            .unwrap_err()
            .code,
        ErrorCode::DocumentGone
    );
}

/// Per-keystroke cost, printed with `--nocapture` (the bound is loose so a debug build passes).
#[test]
fn a_keystroke_preview_is_cheap() {
    let (e, doc) = engine(true);
    let text = "The quick brown fox jumps over the lazy dogs";
    e.layout_edit(&doc, 0, vec![A], text, None, true).unwrap();
    let t = std::time::Instant::now();
    for _ in 0..50 {
        e.layout_edit(&doc, 0, vec![A], text, None, true).unwrap();
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / 50.0;
    eprintln!("layout_edit: {ms:.2} ms per call");
    assert!(ms < 100.0, "{ms} ms");
}
