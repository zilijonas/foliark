//! Edit history and the privacy-wins order (spec §4, §5; NEW-53) through the public engine ops:
//! edits replay from the source, text redactions resolve against the base revision, and export
//! runs edits, then redactions, then sanitization. Exported bytes are judged by independent
//! readers (PyMuPDF, pypdf, pikepdf and the objects oracle), never by the engine that wrote them.
mod common;
use common::{Font, objects_oracle, pdf_bytes, python, run, tmp};
use foliark_engine::{Engine, RenderOptions};
use foliark_model::{
    ChangeResult, ChangeSet, ErrorCode, ExportMode, ExportPolicy, ExportReport, ExtractOptions,
    FontSource, FontStyle, Limits, Rect, Redaction, SearchOptions, TextEdit,
};

/// 300 x 400 page. Paragraph A: display baselines 100 and 114, x 20 to 145. N is the neighbour
/// line (baseline 200).
const PAGE: &str = "BT /F1 10 Tf 20 300 Td (The quick brown fox jumps) Tj ET\n\
    BT /F1 10 Tf 20 286 Td (over the lazy dogs again.) Tj ET\n\
    BT /F1 10 Tf 20 200 Td (Keep this neighbour line) Tj ET";
const A: Rect = rect(10.0, 85.0, 150.0, 37.0);
const N: Rect = rect(10.0, 185.0, 150.0, 20.0);

const fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn sans() -> FontSource {
    FontSource {
        bytes: std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../foliark-text/tests/data/LiberationSans-Regular.ttf"
        ))
        .unwrap(),
        family: "Liberation Sans".into(),
        weight: 400,
        style: FontStyle::Normal,
    }
}

fn engine(bytes: Vec<u8>) -> (Engine, String) {
    let mut e = Engine::new(Limits::default());
    e.register_fonts(vec![sans()]).unwrap();
    let (doc, _) = e.open(bytes, None, false).unwrap();
    (e, doc)
}

fn page() -> Vec<u8> {
    pdf_bytes(&[PAGE], &Font::default(), 0)
}

fn edit(id: &str, rects: &[Rect], text: &str) -> TextEdit {
    TextEdit {
        id: id.into(),
        page_index: 0,
        rects: rects.to_vec(),
        text: text.into(),
        font: None,
    }
}

fn try_changes(
    e: &mut Engine,
    doc: &str,
    base: u32,
    edits: Option<Vec<TextEdit>>,
    reds: Option<Vec<Redaction>>,
) -> Result<ChangeResult, foliark_model::PdfError> {
    let t = e.apply_changes(
        doc,
        ChangeSet {
            base_revision: base,
            text_edits: edits,
            redactions: reds,
        },
    );
    run(e, t)
}

fn changes(
    e: &mut Engine,
    doc: &str,
    edits: Option<Vec<TextEdit>>,
    reds: Option<Vec<Redaction>>,
) -> ChangeResult {
    let base = e.revision(doc).unwrap();
    try_changes(e, doc, base, edits, reds).unwrap()
}

fn text(e: &mut Engine, doc: &str) -> String {
    let t = e.extract_text(doc, 0, ExtractOptions::default());
    run(e, t).unwrap().text
}

fn red(e: &mut Engine, doc: &str, id: &str, word: &str) -> Redaction {
    let start = text(e, doc).find(word).unwrap() as u32;
    Redaction::Text {
        id: id.into(),
        page_index: 0,
        start,
        end: start + word.len() as u32,
        fill: None,
    }
}

fn export(e: &mut Engine, doc: &str) -> (Vec<u8>, ExportReport) {
    let t = e.prepare_export(doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(e, t).unwrap();
    (e.take_export(doc, &plan.plan_id).unwrap(), plan.report)
}

fn render(e: &mut Engine, doc: &str) -> Vec<u8> {
    let t = e.render(doc, 0, RenderOptions::at(2.0));
    run(e, t).unwrap().data
}

fn flat(s: &str) -> String {
    s.split_whitespace().collect()
}

/// What the independent readers find in `bytes`: PyMuPDF text, pypdf text and the number of
/// registered (Liberation Sans) Type0 font objects anywhere in the file.
struct Probe {
    fitz: String,
    pypdf: String,
    fonts: usize,
}

fn probe(name: &str, bytes: &[u8]) -> Probe {
    let path = tmp("edit-history", name);
    std::fs::write(&path, bytes).unwrap();
    let code = "import sys, json, fitz, pypdf, pikepdf\n\
        f = sys.argv[1]\n\
        pdf = pikepdf.open(f)\n\
        fonts = sum(1 for o in pdf.objects if isinstance(o, pikepdf.Dictionary) and o.get('/Subtype') == '/Type0' and 'LiberationSans' in str(o.get('/BaseFont')))\n\
        print(json.dumps({'fitz': ''.join(p.get_text() for p in fitz.open(f)),\n\
          'pypdf': ''.join(p.extract_text() for p in pypdf.PdfReader(f).pages), 'fonts': fonts}))\n";
    let (ok, log) = python(&["-c", code, path.to_str().unwrap()]);
    assert!(ok, "{log}");
    let v: serde_json::Value =
        serde_json::from_str(log.lines().rfind(|l| l.starts_with('{')).expect(&log)).unwrap();
    Probe {
        fitz: flat(v["fitz"].as_str().unwrap()),
        pypdf: flat(v["pypdf"].as_str().unwrap()),
        fonts: v["fonts"].as_u64().unwrap() as usize,
    }
}

/// Both extractors read every `want` and neither reads any `gone`; the objects oracle finds no
/// `gone` anywhere in the file (streams, strings, ToUnicode, raw bytes).
fn assert_file(name: &str, bytes: &[u8], want: &[&str], gone: &[&str]) {
    let p = probe(name, bytes);
    for (who, all) in [("PyMuPDF", &p.fitz), ("pypdf", &p.pypdf)] {
        for w in want {
            assert!(all.contains(&flat(w)), "{who} lost {w:?}: {all}");
        }
        for g in gone {
            assert!(!all.contains(&flat(g)), "{who} still reads {g:?}: {all}");
        }
    }
    let (ok, log) = objects_oracle("edit-history", name, bytes, gone);
    assert!(ok, "{log}");
}

#[test]
fn an_edit_then_a_redaction_of_its_new_word_leaves_the_rest_of_the_edit() {
    let (mut e, doc) = engine(page());
    let new = "Fresh CODENAME alpha text\nsecond line here";
    assert!(changes(&mut e, &doc, Some(vec![edit("e1", &[A], new)]), None).accepted);
    // The range is resolved against revision 1's text, which holds the inserted word.
    let r = red(&mut e, &doc, "r1", "CODENAME");
    let res = changes(&mut e, &doc, None, Some(vec![r]));
    assert!(res.accepted, "{:?}", res.rejected);
    assert_eq!(res.revision, 2);
    let (out, report) = export(&mut e, &doc);
    assert_eq!(report.edits.applied, 1);
    assert!(
        report.redaction.clusters_removed >= 8,
        "{:?}",
        report.redaction
    );
    assert_file(
        "edit-then-redact.pdf",
        &out,
        &[
            "Fresh",
            "alpha text",
            "second line here",
            "Keep this neighbour line",
        ],
        &["CODENAME", "quick", "brown", "lazy"],
    );
}

#[test]
fn a_redaction_stored_before_an_edit_is_refused_or_kept_never_resurrected() {
    // Across ChangeSets: the edit replaces the words the stored redaction targets. The
    // ChangeSet is refused whole (the redaction is named) and the stored list still holds.
    let (mut e, doc) = engine(page());
    let r = red(&mut e, &doc, "r-brown", "brown");
    assert!(changes(&mut e, &doc, None, Some(vec![r])).accepted);
    let before = text(&mut e, &doc);
    let res = changes(
        &mut e,
        &doc,
        Some(vec![edit("e1", &[A], "something else entirely\nhere")]),
        None,
    );
    assert!(!res.accepted);
    assert_eq!(res.revision, 1);
    assert_eq!(res.rejected.len(), 1);
    assert_eq!(res.rejected[0].id, "r-brown");
    let d = res.rejected[0].details.as_ref().unwrap();
    assert_eq!(d.feature.as_deref(), Some("redactionTargetEdited"));
    assert_eq!(text(&mut e, &doc), before);
    let (out, report) = export(&mut e, &doc);
    assert_eq!(report.edits.applied, 0);
    assert_file(
        "refused-edit.pdf",
        &out,
        &["quick", "fox jumps", "lazy dogs", "neighbour"],
        &["brown", "something"],
    );

    // Same ChangeSet: the redaction resolves at the base text (before the edit), then the edit
    // replaces its words, so the pair is refused the same way and nothing is stored.
    let (mut e, doc) = engine(page());
    let r = red(&mut e, &doc, "r-lazy", "lazy");
    let res = changes(
        &mut e,
        &doc,
        Some(vec![edit("e1", &[A], "something else entirely\nhere")]),
        Some(vec![r]),
    );
    assert!(!res.accepted);
    assert_eq!((res.revision, res.rejected[0].id.as_str()), (0, "r-lazy"));
    assert_eq!(
        res.rejected[0].details.as_ref().unwrap().feature.as_deref(),
        Some("redactionTargetEdited")
    );
    assert_eq!(e.committed(&doc).unwrap().edits.len(), 0);
    assert_eq!(e.committed(&doc).unwrap().redactions.len(), 0);

    // The explicit way out: the same ChangeSet that edits the text also drops the redaction.
    let res = changes(
        &mut e,
        &doc,
        Some(vec![edit("e1", &[A], "something else entirely\nhere")]),
        Some(vec![]),
    );
    assert!(res.accepted, "{:?}", res.rejected);
}

#[test]
fn an_edit_that_retypes_a_redacted_word_in_place_cannot_bring_it_back() {
    // "brown" keeps its exact position (the new text only changes what follows), so the stored
    // redaction is found again on the edit's own text. The edit is accepted and the redaction follows it.
    let (mut e, doc) = engine(page());
    let r = red(&mut e, &doc, "r-brown", "brown");
    assert!(changes(&mut e, &doc, None, Some(vec![r])).accepted);
    let res = changes(
        &mut e,
        &doc,
        Some(vec![edit(
            "e1",
            &[A],
            "The quick brown fox leaps\nover the lazy dogs again.",
        )]),
        None,
    );
    // The redaction is re-resolved onto the edit's own clusters and still removes them.
    assert!(res.accepted, "{:?}", res.rejected);
    let (out, _) = export(&mut e, &doc);
    assert_file(
        "retyped.pdf",
        &out,
        &["The quick", "fox leaps", "lazy dogs again", "neighbour"],
        &["brown", "jumps"],
    );
}

#[test]
fn an_edit_elsewhere_keeps_a_stored_redaction_and_both_land_at_export() {
    let (mut e, doc) = engine(page());
    let r = red(&mut e, &doc, "r-brown", "brown");
    assert!(changes(&mut e, &doc, None, Some(vec![r])).accepted);
    let res = changes(
        &mut e,
        &doc,
        Some(vec![edit("e1", &[N], "Replaced neighbour words")]),
        None,
    );
    assert!(res.accepted, "{:?}", res.rejected);
    let (out, report) = export(&mut e, &doc);
    assert_eq!((report.edits.applied, report.redaction.regions), (1, 1));
    assert_file(
        "elsewhere.pdf",
        &out,
        &["The quick", "fox jumps", "Replaced neighbour words"],
        &["brown", "Keep this"],
    );
}

#[test]
fn edit_only_and_sanitize_only_exports_say_what_they_did() {
    let (mut e, doc) = engine(page());
    let (plain, report) = export(&mut e, &doc);
    assert_eq!((report.edits.applied, report.redaction.regions), (0, 0));
    assert_file(
        "plain.pdf",
        &plain,
        &[
            "quick brown fox jumps",
            "lazy dogs again",
            "Keep this neighbour line",
        ],
        &[],
    );

    assert!(
        changes(
            &mut e,
            &doc,
            Some(vec![edit("e1", &[A], "Totally new words\nover two lines")]),
            None
        )
        .accepted
    );
    let (out, report) = export(&mut e, &doc);
    assert_eq!(
        (
            report.edits.applied,
            report.redaction.regions,
            report.redaction.clusters_removed
        ),
        (1, 0, 0)
    );
    // No redaction ran, yet the replaced words are gone from every object: edit removal is
    // privacy-grade by itself.
    assert_file(
        "edit-only.pdf",
        &out,
        &[
            "Totally new words",
            "over two lines",
            "Keep this neighbour line",
        ],
        &["quick", "brown", "jumps", "lazy", "dogs"],
    );
}

#[test]
fn undoing_every_edit_restores_text_render_and_export_exactly() {
    let region = Redaction::Region {
        id: "r".into(),
        page_index: 0,
        rect: N,
        fill: Some(Some([0.0; 3])),
    };
    let (mut fresh, fdoc) = engine(page());
    let src_text = text(&mut fresh, &fdoc);
    let src_render = render(&mut fresh, &fdoc);
    assert!(changes(&mut fresh, &fdoc, None, Some(vec![region.clone()])).accepted);
    let (src_export, _) = export(&mut fresh, &fdoc);
    let (src_plain, _) = {
        let (mut e, d) = engine(page());
        export(&mut e, &d)
    };

    let (mut e, doc) = engine(page());
    let r = changes(
        &mut e,
        &doc,
        Some(vec![edit(
            "e1",
            &[A],
            "A longer replacement\nover two lines",
        )]),
        Some(vec![region]),
    );
    assert!(r.accepted, "{:?}", r.rejected);
    assert_ne!(text(&mut e, &doc), src_text);
    assert_ne!(render(&mut e, &doc), src_render);
    // Undo = resubmit the older (empty) edit list; the redaction list is kept as it is.
    let undo = changes(&mut e, &doc, Some(vec![]), None);
    assert!(undo.accepted && undo.revision == 2, "{:?}", undo.rejected);
    assert_eq!(undo.changed_pages, vec![0]);
    assert_eq!(text(&mut e, &doc), src_text);
    let (out, report) = export(&mut e, &doc);
    assert_eq!(report.edits.applied, 0);
    assert!(
        out == src_export,
        "undone export differs from source export"
    );
    // And with the redaction undone as well, the render is the source render, pixel for pixel,
    // and the export is the source-only export, byte for byte.
    assert!(changes(&mut e, &doc, None, Some(vec![])).accepted);
    assert!(render(&mut e, &doc) == src_render, "render differs");
    let (out, _) = export(&mut e, &doc);
    assert!(
        out == src_plain,
        "restored export differs from source export"
    );
}

#[test]
fn ten_edits_and_undos_leave_one_font_object_and_a_bounded_working_copy() {
    // Ten paragraphs, one per line, 30 pt apart.
    let content: Vec<String> = (0..10)
        .map(|i| {
            format!(
                "BT /F1 10 Tf 20 {} Td (Paragraph number {i} here) Tj ET",
                380 - 30 * i
            )
        })
        .collect();
    let bytes = pdf_bytes(&[&content.join("\n")], &Font::default(), 0);
    let (mut e, doc) = engine(bytes);
    let rect_of = |i: usize| rect(10.0, 10.0 + 30.0 * i as f64, 200.0, 14.0);
    let mut sizes = Vec::new();
    let mut list: Vec<TextEdit> = Vec::new();
    for i in 0..10 {
        list.push(edit(
            &format!("e{i}"),
            &[rect_of(i)],
            &format!("Edited paragraph {i} ąčę"),
        ));
        let r = changes(&mut e, &doc, Some(list.clone()), None);
        assert!(r.accepted, "{i}: {:?}", r.rejected);
        assert_eq!(r.revision, i as u32 + 1);
        sizes.push(
            e.working_copy(&doc)
                .unwrap()
                .reader()
                .pdf()
                .data()
                .as_ref()
                .len(),
        );
    }
    let wc = e
        .working_copy(&doc)
        .unwrap()
        .reader()
        .pdf()
        .data()
        .as_ref()
        .to_vec();
    assert_eq!(probe("accumulated.pdf", &wc).fonts, 1, "{sizes:?}");
    // Each added edit costs roughly its own text, not another font subset.
    let per_edit = (sizes[9] - sizes[0]) / 9;
    assert!(per_edit < 1_000, "{sizes:?}");

    // Ten more revisions of undo and redo of the same list: bytes after every redo are identical,
    // so the working copy cannot creep.
    let full = sizes[9];
    for k in 0..10u32 {
        assert!(changes(&mut e, &doc, Some(vec![]), None).accepted);
        assert!(changes(&mut e, &doc, Some(list.clone()), None).accepted);
        let now = e
            .working_copy(&doc)
            .unwrap()
            .reader()
            .pdf()
            .data()
            .as_ref()
            .len();
        assert_eq!(now, full, "round {k}");
    }
    assert_eq!(e.revision(&doc).unwrap(), 30);
    let (out, report) = export(&mut e, &doc);
    assert_eq!(report.edits.applied, 10);
    assert_eq!(probe("accumulated.out.pdf", &out).fonts, 1);
}

#[test]
fn a_later_edit_over_earlier_text_replaces_it_and_ids_cannot_repeat() {
    // Spec §4: each edit's rects are evaluated on the state after the previous edits, so an edit
    // over what an earlier edit drew replaces it. Overlap is not an error: nothing is
    // merged and the earlier text does not linger anywhere in the export.
    let (mut e, doc) = engine(page());
    let res = changes(
        &mut e,
        &doc,
        Some(vec![
            edit("first", &[A], "Primary draft of text"),
            edit("second", &[A], "Second version"),
            edit("other", &[N], "Other words"),
        ]),
        None,
    );
    assert!(res.accepted, "{:?}", res.rejected);
    assert_eq!(
        res.layouts
            .iter()
            .map(|l| l.id.as_str())
            .collect::<Vec<_>>(),
        ["first", "second", "other"]
    );
    let now = text(&mut e, &doc);
    assert!(
        now.contains("Second version") && !now.contains("Primary draft"),
        "{now}"
    );
    let (out, report) = export(&mut e, &doc);
    assert_eq!(report.edits.applied, 3);
    assert_file(
        "replaced.pdf",
        &out,
        &["Second version", "Other words"],
        &["Primary", "quick", "brown", "Keep this"],
    );

    // An id names one operation: a repeated edit id, or an edit id reused by a redaction, is
    // refused with the id named and nothing changes.
    let dup = changes(
        &mut e,
        &doc,
        Some(vec![edit("a", &[A], "x"), edit("a", &[N], "y")]),
        None,
    );
    assert_eq!(
        (
            dup.accepted,
            dup.rejected[0].id.as_str(),
            dup.rejected[0].code
        ),
        (false, "a", ErrorCode::InvalidArgument)
    );
    let r = Redaction::Region {
        id: "first".into(),
        page_index: 0,
        rect: N,
        fill: None,
    };
    let clash = changes(&mut e, &doc, None, Some(vec![r]));
    assert_eq!(
        (clash.accepted, clash.rejected[0].id.as_str()),
        (false, "first")
    );
    assert_eq!(e.revision(&doc).unwrap(), 1);
}

#[test]
fn results_from_an_older_revision_are_refused_and_fresh_reads_see_the_new_layout() {
    let (mut e, doc) = engine(page());
    let t = e.search(&doc, "quick", SearchOptions::default());
    let old_hits = run(&mut e, t).unwrap();
    assert_eq!((old_hits.len(), old_hits[0].revision), (1, 0));
    let old_quads = old_hits[0].range.quads.clone();
    let old_range = (old_hits[0].range.start, old_hits[0].range.end);
    let old_preview = e.layout_edit(&doc, 0, vec![A], "x", None, false).unwrap();
    let t = e.prepare_export(&doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(&mut e, t).unwrap();

    let res = changes(
        &mut e,
        &doc,
        Some(vec![edit("e1", &[A], "Brand new first line\nand a second")]),
        None,
    );
    assert!(res.accepted, "{:?}", res.rejected);

    // The plan is stale, a ChangeSet based on the old revision is stale, whether or not it
    // carries text offsets taken from the old hit.
    assert_eq!(
        e.take_export(&doc, &plan.plan_id).unwrap_err().code,
        ErrorCode::StalePlan
    );
    let stale = Redaction::Text {
        id: "r".into(),
        page_index: 0,
        start: old_range.0,
        end: old_range.1,
        fill: None,
    };
    assert_eq!(
        try_changes(&mut e, &doc, 0, None, Some(vec![stale.clone()]))
            .unwrap_err()
            .code,
        ErrorCode::StaleRevision
    );
    assert_eq!(e.committed(&doc).unwrap().redactions.len(), 0);

    // Fresh reads: the old word is gone, the old offsets now name other text, the preview is
    // stamped with the new revision and a new fingerprint.
    let t = e.search(&doc, "quick", SearchOptions::default());
    assert!(run(&mut e, t).unwrap().is_empty());
    let t = e.search(&doc, "Brand", SearchOptions::default());
    let new_hits = run(&mut e, t).unwrap();
    assert_eq!((new_hits.len(), new_hits[0].revision), (1, 1));
    assert_ne!(new_hits[0].range.quads, old_quads);
    let now = e.quads_for(&doc, 0, old_range.0, old_range.1).unwrap();
    assert_ne!(now.text, "quick");
    let preview = e.layout_edit(&doc, 0, vec![A], "x", None, false).unwrap();
    assert_eq!((old_preview.revision, preview.revision), (0, 1));
    assert_ne!(old_preview.fingerprint, preview.fingerprint);
    let t = e.extract_text(&doc, 0, ExtractOptions::default());
    let words = run(&mut e, t).unwrap().words;
    assert!(words.iter().any(|w| w.text == "Brand"));
    assert!(!words.iter().any(|w| w.text == "quick"));

    // Re-selected at the new revision, the same kind of ChangeSet is accepted.
    let fresh = red(&mut e, &doc, "r", "Brand");
    assert!(
        try_changes(&mut e, &doc, 1, None, Some(vec![fresh]))
            .unwrap()
            .accepted
    );
}
