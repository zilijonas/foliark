//! HDDN-201: a right-aligned italic amendment note above an indented regular body, offered as one
//! block (ADR 0012). The note is a segment whose own left edge is far right of the paragraph's;
//! the text HDDN sends carries that indent as leading spaces of the whole paragraph. Editing the
//! note must lay it out from the paragraph's left edge, so those spaces land where the page draws
//! the note: it fits on its one line and ends inside the body's right edge, where it once
//! measured them from the note's own edge, wrapped to extra lines and was refused.
mod common;
use common::{fixture, run};
use foliark_engine::Engine;
use foliark_model::{ChangeSet, ExtractOptions, Limits, Rect, TextEdit, TextPage};

/// The block HDDN offers: the note (baseline 100) and the body's three rows (107, 114, 121).
const BLOCK: Rect = Rect {
    x: 15.0,
    y: 94.0,
    width: 160.0,
    height: 31.0,
};
/// The body's right edge; the body's rows end there; the note ends just short of it.
const RIGHT: f64 = 170.0;

fn open() -> (Engine, String, String) {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e
        .open(fixture("generated/word-right-note-body.pdf"), None, false)
        .unwrap();
    let info = e
        .prepare_paragraph(&doc, 0, vec![BLOCK], None, false)
        .unwrap();
    let original = info.original_text.unwrap();
    (e, doc, original)
}

fn page(e: &mut Engine, doc: &str) -> TextPage {
    let t = e.extract_text(doc, 0, ExtractOptions::default());
    run(e, t).unwrap()
}

#[test]
fn editing_the_note_keeps_it_on_its_line_inside_the_right_edge_and_the_body_as_it_is() {
    let (mut e, doc, original) = open();
    let before = page(&mut e, &doc);
    let text = original.replace("text", "tent");
    assert_ne!(text, original);

    let p = e
        .layout_edit(&doc, 0, vec![BLOCK], &text, None, false)
        .unwrap();
    assert!(p.fits && p.needed_lines == 1 && p.lines.len() == 1, "{p:?}");
    assert!(p.lines[0].text.ends_with("(Amended tent)"), "{p:?}");
    assert!(p.lines[0].x + p.lines[0].width <= RIGHT + 1e-6, "{p:?}");

    let t = e.apply_changes(
        &doc,
        ChangeSet {
            base_revision: e.revision(&doc).unwrap(),
            text_edits: Some(vec![TextEdit {
                id: "e".into(),
                page_index: 0,
                rects: vec![BLOCK],
                text,
                font: None,
                reflow: None,
            }]),
            redactions: None,
        },
    );
    let c = run(&mut e, t).unwrap();
    assert!(c.accepted, "{:?}", c.rejected);
    assert_eq!(c.layouts[0].fingerprint, p.fingerprint);

    let after = page(&mut e, &doc);
    let words = |p: &TextPage, from: f64| -> Vec<(String, f64)> {
        p.words
            .iter()
            .filter(|w| w.bounds.y > from)
            .map(|w| (w.text.clone(), w.bounds.x + w.bounds.width))
            .collect()
    };
    // The body (rows below the note's) is untouched; the note's words end by the right edge.
    assert_eq!(words(&after, 103.0), words(&before, 103.0));
    let note: Vec<_> = after
        .words
        .iter()
        .filter(|w| w.text == "tent)" || w.text == "(Amended")
        .collect();
    assert_eq!(note.len(), 2, "{:?}", after.words);
    assert!(
        note.iter()
            .all(|w| w.bounds.x + w.bounds.width <= RIGHT + 1e-6)
    );
}

#[test]
fn a_note_too_long_for_its_line_is_still_refused() {
    let (e, doc, original) = open();
    let text = original.replace(
        "(Amended text)",
        "(Amended text text text text text text text)",
    );
    let p = e
        .layout_edit(&doc, 0, vec![BLOCK], &text, None, false)
        .unwrap();
    assert!(!p.fits && p.needed_lines > p.max_lines, "{p:?}");
}

#[test]
fn editing_the_body_leaves_the_note() {
    let (e, doc, original) = open();
    let p = e
        .layout_edit(
            &doc,
            0,
            vec![BLOCK],
            &original.replace("charlie", "charles"),
            None,
            false,
        )
        .unwrap();
    assert!(p.fits && p.lines.len() == 3, "{p:?}");
}
