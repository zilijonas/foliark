//! Unchanged text fits its own edit box with its own line breaks (ADR 0011; NEW-104). The fixture
//! is the iOS Quartz shape: /W widths 1/1000 em wider than the advances the page draws, an ogonek
//! drawn as a zero-advance mark after its base, and no precomposed letter in the subset. No font is
//! registered, so the source font is the only one. The export is judged by the objects oracle.
mod common;
use common::{fixture, objects_oracle, run};
use foliark_engine::Engine;
use foliark_model::{
    ChangeResult, ChangeSet, ErrorCode, ExportMode, ExportPolicy, Limits, Rect, RunSource, TextEdit,
};

const FILE: &str = "generated/quartz-type0-tight-widths.pdf";
/// Display space, size 10: three rows at baselines 100, 114, 128 that each fill the box.
const PARAGRAPH: Rect = rect(15.0, 90.0, 120.0, 42.0);
/// `KILO` at baseline 200, a word beside it.
const WORD: Rect = rect(15.0, 190.0, 50.0, 12.0);
/// `ZĄSIS` at baseline 230: `A` then a zero-advance U+0328.
const OGONEK: Rect = rect(15.0, 220.0, 70.0, 12.0);

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

fn commit(e: &mut Engine, doc: &str, edits: &[(Rect, &str)]) -> ChangeResult {
    let edits = edits
        .iter()
        .enumerate()
        .map(|(i, (r, t))| TextEdit {
            id: format!("e{i}"),
            page_index: 0,
            rects: vec![*r],
            text: (*t).into(),
            font: None,
        })
        .collect();
    let t = e.apply_changes(
        doc,
        ChangeSet {
            base_revision: e.revision(doc).unwrap(),
            text_edits: Some(edits),
            redactions: None,
        },
    );
    run(e, t).unwrap()
}

#[test]
fn unchanged_text_keeps_its_breaks_in_the_source_font_and_the_preview_is_the_commit() {
    for (r, rows) in [
        (
            PARAGRAPH,
            &["ALPHA BRAVO", "DELTA ECHOS", "GOLFS HOTEL"][..],
        ),
        (WORD, &["KILO"][..]),
        (OGONEK, &["Z\u{104}SIS"][..]),
    ] {
        let (mut e, doc) = open();
        let info = e.prepare_paragraph(&doc, 0, vec![r], None, false).unwrap();
        let text = info.original_text.clone().unwrap();
        let p = e.layout_edit(&doc, 0, vec![r], &text, None, false).unwrap();
        let lines: Vec<&str> = p.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(lines, rows, "{info:?}");
        assert!(p.fits && p.max_lines as usize >= rows.len(), "{p:?}");
        assert!(!p.approximate);
        assert!(p.runs.iter().all(|x| x.source == RunSource::Embedded));
        // No line is wider than the box the page gives the paragraph.
        assert!(p.lines.iter().all(|l| l.width <= info.wrap_width + 1e-6));

        let c = commit(&mut e, &doc, &[(r, text.as_str())]);
        assert!(c.accepted, "{:?}", c.rejected);
        assert_eq!(c.layouts[0].fingerprint, p.fingerprint);
        assert_eq!(c.layouts[0].lines as usize, rows.len());
        assert!(!c.layouts[0].approximate);
    }
}

#[test]
fn longer_text_still_does_not_fit() {
    let (mut e, doc) = open();
    for (r, text, needed, max) in [
        (PARAGRAPH, "ALPHA BRAVO DELTA ECHOS GOLFS HOTEL LIMA", 4, 3),
        (PARAGRAPH, "ALPHA BRAVOS DELTA ECHOS GOLFS HOTEL", 4, 3),
        // A lone word beside text may use the room up to it (ADR 0011 amendment), not more.
        (
            WORD,
            "KILOS ALPHA BRAVO DELTA ECHOS GOLFS HOTEL LIMA MIKES",
            3,
            2,
        ),
    ] {
        let p = e.layout_edit(&doc, 0, vec![r], text, None, false).unwrap();
        assert!(!p.fits, "{text}: {p:?}");
        assert_eq!((p.needed_lines, p.max_lines), (needed, max), "{text}");
        let c = commit(&mut e, &doc, &[(r, text)]);
        assert_eq!(c.rejected[0].code, ErrorCode::EditNoFit, "{text}");
    }
}

#[test]
fn an_edit_beside_a_decomposed_letter_exports_without_the_old_word() {
    let input = fixture(FILE);
    // The oracle reads these glyphs (through the program's cmap) in the input: it can see a leak.
    let (ok, log) = objects_oracle("paragraph-fit", "input.pdf", &input, &["HOTEL"]);
    assert!(!ok && log.contains("TEXT_GLYPH"), "{log}");

    let (mut e, doc) = open();
    let c = commit(
        &mut e,
        &doc,
        &[
            (PARAGRAPH, "ALPHA BRAVO DELTA ECHOS GOLFS"),
            (OGONEK, "Z\u{104}SIS SAZ\u{104}"),
        ],
    );
    assert!(c.accepted, "{:?}", c.rejected);
    assert!(c.layouts.iter().all(|l| !l.approximate));
    let t = e.prepare_export(&doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(&mut e, t).unwrap();
    assert_eq!(
        (plan.report.edits.applied, plan.report.edits.approximate),
        (2, 0)
    );
    let out = e.take_export(&doc, &plan.plan_id).unwrap();
    let (ok, log) = objects_oracle("paragraph-fit", "edited.pdf", &out, &["HOTEL"]);
    assert!(ok, "{log}");
}
