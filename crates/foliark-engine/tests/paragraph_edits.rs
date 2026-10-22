//! Paragraph edits (spec §4, §6.3; NEW-51) through the public engine ops. The output bytes are
//! judged by independent readers (pypdf and PyMuPDF text, rendering, decoded streams, the
//! objects and fidelity oracles in tests/oracles), never by the engine that wrote them.
mod common;
use common::{Font, fidelity_oracle, fixture, objects_oracle, pdf_bytes, python, run, tmp};
use foliark_engine::edit::ParagraphApplier;
use foliark_engine::{Applied, Document, EditApplier, EditContext, Engine};
use foliark_model::{
    ChangeResult, ChangeSet, ErrorCode, ExportMode, ExportPolicy, ExportReport, ExtractOptions,
    FontSource, FontStyle, Limits, LossCode, Rect, Redaction, RejectedChange, RunSource, TextEdit,
};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 300 x 400 page. Paragraph A: display baselines 100 and 114 (user 300, 286), x 20 to 145 on a
/// pale blue background rectangle (display y 85 to 145) with a rule below it. Line N is the
/// neighbour (baseline 200).
const PAGE: &str = "q 0.8 0.9 1 rg 10 255 220 60 re f Q\n\
    q 0 G 2 w 10 240 m 290 240 l S Q\n\
    BT /F1 10 Tf 20 300 Td (The quick brown fox jumps) Tj ET\n\
    BT /F1 10 Tf 20 286 Td (over the lazy dogs again.) Tj ET\n\
    BT /F1 10 Tf 20 200 Td (Keep this neighbour line) Tj ET";
const OLD: [&str; 6] = ["quick", "brown", "jumps", "lazy", "dogs", "again"];
/// Both lines of paragraph A, display space.
const A: Rect = rect(10.0, 85.0, 150.0, 37.0);

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

fn engine(bytes: Vec<u8>, fonts: bool) -> (Engine, String) {
    let mut e = Engine::new(Limits::default());
    if fonts {
        e.register_fonts(vec![sans()]).unwrap();
    }
    let (doc, _) = e.open(bytes, None, false).unwrap();
    (e, doc)
}

fn page() -> Vec<u8> {
    pdf_bytes(&[PAGE], &Font::default(), 0)
}

fn edit(id: &str, page: u32, rects: &[Rect], text: &str) -> TextEdit {
    TextEdit {
        id: id.into(),
        page_index: page,
        rects: rects.to_vec(),
        text: text.into(),
        font: None,
    }
}

fn changes(
    e: &mut Engine,
    doc: &str,
    edits: Option<Vec<TextEdit>>,
    reds: Option<Vec<Redaction>>,
) -> ChangeResult {
    let base = e.revision(doc).unwrap();
    let t = e.apply_changes(
        doc,
        ChangeSet {
            base_revision: base,
            text_edits: edits,
            redactions: reds,
        },
    );
    run(e, t).unwrap()
}

fn text(e: &mut Engine, doc: &str, page: u32) -> String {
    let t = e.extract_text(doc, page, ExtractOptions::default());
    run(e, t).unwrap().text
}

fn export(e: &mut Engine, doc: &str) -> (Vec<u8>, ExportReport) {
    let t = e.prepare_export(doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(e, t).unwrap();
    (e.take_export(doc, &plan.plan_id).unwrap(), plan.report)
}

struct Probe {
    fitz: Vec<String>,
    pypdf: Vec<String>,
    fonts: Vec<String>,
    images: Vec<usize>,
    /// Share of dark pixels in the `--ink` region of page 0, and the RGB at `--px`.
    ink: f64,
    px: [u8; 3],
}

const PROBE: &str = "
import sys, json, fitz, pypdf
f, ink, px = sys.argv[1], [float(v) for v in sys.argv[2].split(',')], [float(v) for v in sys.argv[3].split(',')]
d = fitz.open(f)
pix = d[0].get_pixmap(dpi=144, clip=fitz.Rect(*ink))
dark = sum(1 for i in range(0, len(pix.samples), pix.n) if max(pix.samples[i:i+3]) < 110)
p1 = d[0].get_pixmap(dpi=72, clip=fitz.Rect(px[0], px[1], px[0] + 1, px[1] + 1))
print(json.dumps({
  'fitz': [p.get_text() for p in d],
  'pypdf': [p.extract_text() for p in pypdf.PdfReader(f).pages],
  'fonts': sorted({x[3] for p in d for x in p.get_fonts()}),
  'images': [len(p.get_images()) for p in d],
  'ink': dark / (pix.width * pix.height),
  'px': list(p1.samples[:3])}))
";

/// `ink`: display rect whose dark-pixel share is measured; `px`: a display point whose colour is read.
fn probe(name: &str, bytes: &[u8], ink: Rect, px: (f64, f64)) -> Probe {
    let path = tmp("paragraph-edits", name);
    std::fs::write(&path, bytes).unwrap();
    let ink = format!(
        "{},{},{},{}",
        ink.x,
        ink.y,
        ink.x + ink.width,
        ink.y + ink.height
    );
    let (ok, log) = python(&[
        "-c",
        PROBE,
        path.to_str().unwrap(),
        &ink,
        &format!("{},{}", px.0, px.1),
    ]);
    assert!(ok, "{log}");
    let line = log.lines().rfind(|l| l.starts_with('{')).expect(&log);
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    let list = |k: &str| v[k].as_array().unwrap().clone();
    let strings = |k: &str| {
        list(k)
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect()
    };
    Probe {
        fitz: strings("fitz"),
        pypdf: strings("pypdf"),
        fonts: strings("fonts"),
        images: list("images")
            .iter()
            .map(|n| n.as_u64().unwrap() as usize)
            .collect(),
        ink: v["ink"].as_f64().unwrap(),
        px: [0, 1, 2].map(|i| v["px"][i].as_u64().unwrap() as u8),
    }
}

fn flat(s: &str) -> String {
    s.split_whitespace().collect()
}

/// Both extractors read `want` and neither reads any of `gone`.
fn assert_text(p: &Probe, want: &[&str], gone: &[&str]) {
    for (who, pages) in [("PyMuPDF", &p.fitz), ("pypdf", &p.pypdf)] {
        let all = flat(&pages.concat());
        for w in want {
            assert!(all.contains(&flat(w)), "{who} lost {w:?}: {all}");
        }
        for g in gone {
            assert!(!all.contains(&flat(g)), "{who} still reads {g:?}: {all}");
        }
    }
}

#[test]
fn a_paragraph_is_replaced_with_registered_glyphs_and_everything_else_stays() {
    let input = page();
    let (mut e, doc) = engine(input.clone(), true);
    let new = "ąčęėįšųūž testing\nŽaliasis rašalas";
    let r = changes(&mut e, &doc, Some(vec![edit("e1", 0, &[A], new)]), None);
    assert!(r.accepted, "{:?}", r.rejected);
    assert_eq!((r.revision, r.changed_pages.clone()), (1, vec![0]));
    let l = &r.layouts[0];
    assert_eq!((l.id.as_str(), l.lines, l.approximate), ("e1", 2, true));
    // Source glyphs are used where the page proves them, the registered font for the rest.
    let sources: Vec<_> = l.runs.iter().map(|x| x.source).collect();
    assert!(sources.contains(&RunSource::Registered) && sources.contains(&RunSource::Embedded));

    // The engine's own reading of the new revision.
    let t = text(&mut e, &doc, 0);
    assert!(
        t.contains("ąčęėįšųūž testing") && t.contains("Žaliasis rašalas"),
        "{t}"
    );
    assert!(
        OLD.iter().all(|w| !t.contains(w)) && t.contains("Keep this neighbour line"),
        "{t}"
    );

    let (out, report) = export(&mut e, &doc);
    assert_eq!((report.edits.applied, report.edits.approximate), (1, 1));
    let p = probe(
        "replace.out.pdf",
        &out,
        rect(15.0, 88.0, 130.0, 36.0),
        (200.0, 130.0),
    );
    assert_text(
        &p,
        &[
            "ąčęėįšųūž testing",
            "Žaliasis rašalas",
            "Keep this neighbour line",
        ],
        &OLD,
    );
    assert!(
        p.fonts.iter().any(|f| f.contains("LiberationSans")),
        "{:?}",
        p.fonts
    );
    assert!(p.ink > 0.01, "the new text must be drawn, ink {}", p.ink);
    // No guessed white cover: the background behind the paragraph is still there.
    assert_eq!(
        p.px,
        [204, 229, 255],
        "background colour at an empty point of the paragraph"
    );

    let (ok, log) = objects_oracle("paragraph-edits", "replace.pdf", &out, &OLD);
    assert!(ok, "objects.py: {log}");
    // Everything outside the paragraph (neighbour line, background, rule) is where it was.
    let (ok, log) = fidelity_oracle(
        "paragraph-edits",
        "replace",
        &input,
        &out,
        &[(0, 10.0, 85.0, 230.0, 146.0)],
        &["Keep this neighbour line"],
    );
    assert!(ok, "fidelity.py: {log}");
}

#[test]
fn covered_characters_keep_the_source_font() {
    let (mut e, doc) = engine(page(), true);
    let line2 = rect(10.0, 104.0, 150.0, 14.0);
    let r = changes(
        &mut e,
        &doc,
        Some(vec![edit("e1", 0, &[line2], "the dog one")]),
        None,
    );
    assert!(r.accepted, "{:?}", r.rejected);
    let l = &r.layouts[0];
    assert_eq!((l.lines, l.approximate), (1, false));
    assert_eq!(
        l.runs
            .iter()
            .map(|x| (x.source, x.clusters))
            .collect::<Vec<_>>(),
        [(RunSource::Embedded, 11)]
    );
    let (out, report) = export(&mut e, &doc);
    assert_eq!((report.edits.applied, report.edits.approximate), (1, 0));
    let p = probe(
        "source.out.pdf",
        &out,
        rect(15.0, 104.0, 130.0, 14.0),
        (200.0, 130.0),
    );
    assert_text(
        &p,
        &["The quick brown fox jumps", "the dog one"],
        &["lazy", "dogs", "again"],
    );
    assert_eq!(p.fonts, ["Helvetica"], "no registered font may be written");
    assert!(p.ink > 0.01);
}

#[test]
fn overflow_and_missing_fonts_are_refused_and_change_nothing() {
    let (mut e, doc) = engine(page(), true);
    let before = text(&mut e, &doc, 0);
    let long = "wordwordword ".repeat(60);
    let r = changes(&mut e, &doc, Some(vec![edit("big", 0, &[A], &long)]), None);
    assert!(!r.accepted && r.revision == 0);
    let d = r.rejected[0].details.as_ref().unwrap();
    assert_eq!(r.rejected[0].code, ErrorCode::EditNoFit);
    assert!(d.needed_lines.unwrap() > d.max_lines.unwrap(), "{d:?}");
    assert_eq!(d.edit_id.as_deref(), Some("big"));
    // A word wider than the paragraph is a no-fit too.
    let wide = "x".repeat(200);
    let r = changes(&mut e, &doc, Some(vec![edit("wide", 0, &[A], &wide)]), None);
    assert_eq!(r.rejected[0].code, ErrorCode::EditNoFit);
    // One bad edit next to a good one: the good one is not kept.
    let both = vec![edit("ok", 0, &[A], "fine"), edit("bad", 0, &[A], &long)];
    assert!(!changes(&mut e, &doc, Some(both), None).accepted);
    assert_eq!(
        (e.revision(&doc).unwrap(), text(&mut e, &doc, 0)),
        (0, before.clone())
    );

    // No registered font and a character the page never drew.
    let (mut bare, doc2) = engine(page(), false);
    let r = changes(
        &mut bare,
        &doc2,
        Some(vec![edit("ž", 0, &[A], "ąžuolas")]),
        None,
    );
    assert_eq!(r.rejected[0].code, ErrorCode::EditNoFont);
    assert!(
        r.rejected[0]
            .details
            .as_ref()
            .unwrap()
            .chars
            .as_deref()
            .unwrap()
            .contains('ą')
    );
    // No text under the rects, and text that cannot be set at all.
    let r = changes(
        &mut e,
        &doc,
        Some(vec![edit("n", 0, &[rect(0.0, 300.0, 20.0, 20.0)], "x")]),
        None,
    );
    assert_eq!(r.rejected[0].code, ErrorCode::EditNoStyle);
    let r = changes(&mut e, &doc, Some(vec![edit("h", 0, &[A], "שלום")]), None);
    assert_eq!(r.rejected[0].code, ErrorCode::EditUnsupportedScript);
    assert_eq!(text(&mut e, &doc, 0), before);
}

#[test]
fn an_empty_replacement_deletes_the_paragraph_text_only() {
    let input = page();
    let (mut e, doc) = engine(input.clone(), true);
    let r = changes(&mut e, &doc, Some(vec![edit("del", 0, &[A], "")]), None);
    assert!(r.accepted, "{:?}", r.rejected);
    assert_eq!((r.layouts[0].lines, r.layouts[0].approximate), (0, false));
    let (out, report) = export(&mut e, &doc);
    assert_eq!(report.edits.applied, 1);
    let p = probe(
        "delete.out.pdf",
        &out,
        rect(15.0, 88.0, 130.0, 36.0),
        (200.0, 130.0),
    );
    assert_text(
        &p,
        &["Keep this neighbour line"],
        &["The", "quick", "over", "dogs", "again"],
    );
    assert!(
        p.ink < 0.001,
        "nothing may be drawn where the paragraph was: {}",
        p.ink
    );
    assert_eq!(p.px, [204, 229, 255]);
    let (ok, log) = objects_oracle(
        "paragraph-edits",
        "delete.pdf",
        &out,
        &["quick", "jumps", "dogs"],
    );
    assert!(ok, "{log}");
    let (ok, log) = fidelity_oracle(
        "paragraph-edits",
        "delete",
        &input,
        &out,
        &[(0, 10.0, 85.0, 230.0, 146.0)],
        &[],
    );
    assert!(ok, "{log}");
}

#[test]
fn a_later_edit_sees_the_state_after_the_earlier_ones() {
    let (mut e, doc) = engine(page(), true);
    // Edit 1 grows the paragraph to three lines; the third line sits where the page was empty.
    let third = rect(10.0, 119.0, 150.0, 12.0);
    let first = edit("e1", 0, &[A], "Alpha line one\nBeta line two\nGamma third");
    let second = edit("e2", 0, &[third], "Delta replaced");
    // Alone, the second edit selects nothing on the source.
    let alone = changes(&mut e, &doc, Some(vec![second.clone()]), None);
    assert_eq!(alone.rejected[0].code, ErrorCode::EditNoStyle);
    let r = changes(&mut e, &doc, Some(vec![first.clone(), second]), None);
    assert!(r.accepted, "{:?}", r.rejected);
    assert_eq!(
        r.layouts
            .iter()
            .map(|l| (l.id.as_str(), l.lines))
            .collect::<Vec<_>>(),
        [("e1", 3), ("e2", 1)]
    );
    let (out, report) = export(&mut e, &doc);
    assert_eq!(report.edits.applied, 2);
    let p = probe(
        "two.out.pdf",
        &out,
        rect(15.0, 88.0, 130.0, 40.0),
        (200.0, 140.0),
    );
    assert_text(
        &p,
        &[
            "Alpha line one",
            "Beta line two",
            "Delta replaced",
            "Keep this neighbour line",
        ],
        &["Gamma", "quick", "lazy"],
    );
    // Replacing the list by its first edit goes back to the Gamma line (replay from the source).
    let r = changes(&mut e, &doc, Some(vec![first]), None);
    assert!(r.accepted && r.revision == 2);
    let t = text(&mut e, &doc, 0);
    assert!(t.contains("Gamma third") && !t.contains("Delta"), "{t}");
}

#[test]
fn text_set_on_a_rotated_page_stands_where_the_paragraph_was() {
    // Text running along user +y reads upright on a page turned 90 degrees.
    let content = "BT /F1 10 Tf 0 1 -1 0 100 50 Tm (turned paragraph here) Tj ET";
    let (mut e, doc) = engine(pdf_bytes(&[content], &Font::default(), 90), true);
    let t = e.extract_text(&doc, 0, ExtractOptions::default());
    let words = run(&mut e, t).unwrap().words;
    let b = words.iter().fold(words[0].bounds, |a, w| {
        let (x0, y0) = (a.x.min(w.bounds.x), a.y.min(w.bounds.y));
        let (x1, y1) = (
            (a.x + a.width).max(w.bounds.x + w.bounds.width),
            (a.y + a.height).max(w.bounds.y + w.bounds.height),
        );
        rect(x0, y0, x1 - x0, y1 - y0)
    });
    let sel = rect(b.x - 1.0, b.y - 1.0, b.width + 2.0, b.height + 2.0);
    let before = e
        .prepare_paragraph(&doc, 0, vec![sel], None, false)
        .unwrap();
    let r = changes(
        &mut e,
        &doc,
        Some(vec![edit("e", 0, &[sel], "other words")]),
        None,
    );
    assert!(r.accepted, "{:?}", r.rejected);
    // Reading order on a turned page follows the unrotated page, so compare the word set.
    let mut got: Vec<_> = text(&mut e, &doc, 0)
        .split_whitespace()
        .map(String::from)
        .collect();
    got.sort_unstable();
    assert_eq!(got, ["other", "words"]);
    let after = e
        .prepare_paragraph(&doc, 0, vec![sel], None, false)
        .unwrap();
    assert_eq!(after.original_text.as_deref(), Some("other words"));
    for (a, b) in [
        (before.origin_x, after.origin_x),
        (before.first_baseline, after.first_baseline),
        (before.font_size, after.font_size),
    ] {
        assert!((a - b).abs() < 0.01, "{before:?} vs {after:?}");
    }
    assert_eq!(after.revision, 1);
}

fn redact(e: &mut Engine, doc: &str, word: &str) -> ChangeResult {
    let start = text(e, doc, 0).find(word).unwrap() as u32;
    let red = Redaction::Text {
        id: format!("r-{word}"),
        page_index: 0,
        start,
        end: start + word.len() as u32,
        fill: None,
    };
    changes(e, doc, None, Some(vec![red]))
}

#[test]
fn a_stored_text_redaction_survives_an_edit_and_privacy_wins_at_export() {
    let (mut e, doc) = engine(page(), true);
    assert!(redact(&mut e, &doc, "neighbour").accepted);
    // The edit rewrites the file; the stored clusters must be found again in the new one.
    let r = changes(
        &mut e,
        &doc,
        Some(vec![edit("e1", 0, &[A], "ąčęėįšųūž testing")]),
        None,
    );
    assert!(r.accepted, "{:?}", r.rejected);
    let c = e.committed(&doc).unwrap();
    assert_eq!(
        (
            c.edits.len(),
            c.redactions.len(),
            c.redactions[0].must_remove.len()
        ),
        (1, 1, 9)
    );
    let (out, report) = export(&mut e, &doc);
    assert_eq!(
        (report.edits.applied, report.redaction.clusters_removed),
        (1, 9)
    );
    let mut gone = OLD.to_vec();
    gone.push("neighbour");
    let p = probe(
        "redact.out.pdf",
        &out,
        rect(15.0, 88.0, 130.0, 36.0),
        (200.0, 130.0),
    );
    assert_text(&p, &["ąčęėįšųūž testing", "Keep this", "line"], &gone);
    let (ok, log) = objects_oracle("paragraph-edits", "redact.pdf", &out, &gone);
    assert!(ok, "{log}");

    // The same redaction submitted in the same ChangeSet as the edit, and an edit list that is
    // cleared again, keep the redaction too.
    let (mut e2, doc2) = engine(page(), true);
    let start = text(&mut e2, &doc2, 0).find("neighbour").unwrap() as u32;
    let red = Redaction::Text {
        id: "r".into(),
        page_index: 0,
        start,
        end: start + 9,
        fill: None,
    };
    let r = changes(
        &mut e2,
        &doc2,
        Some(vec![edit("e1", 0, &[A], "x")]),
        Some(vec![red]),
    );
    assert!(r.accepted, "{:?}", r.rejected);
    assert!(changes(&mut e2, &doc2, Some(vec![]), None).accepted);
    let (out, _) = export(&mut e2, &doc2);
    let p = probe(
        "redact2.out.pdf",
        &out,
        rect(15.0, 88.0, 130.0, 36.0),
        (200.0, 130.0),
    );
    assert_text(&p, &["quick brown fox jumps", "Keep this"], &["neighbour"]);
}

#[test]
fn an_edit_that_removes_text_a_redaction_targets_is_refused() {
    let (mut e, doc) = engine(page(), true);
    assert!(redact(&mut e, &doc, "brown").accepted);
    let before = text(&mut e, &doc, 0);
    let r = changes(
        &mut e,
        &doc,
        Some(vec![edit("e1", 0, &[A], "something else")]),
        None,
    );
    assert!(!r.accepted);
    assert_eq!(r.rejected[0].id, "r-brown");
    let d = r.rejected[0].details.as_ref().unwrap();
    assert_eq!(
        (d.feature.as_deref(), d.pages.as_deref()),
        (Some("redactionTargetEdited"), Some(&[0][..]))
    );
    assert_eq!(
        (e.revision(&doc).unwrap(), text(&mut e, &doc, 0)),
        (1, before)
    );
}

#[test]
fn an_edit_on_a_page_with_images_keeps_them_and_a_font_program_keeps_reused_glyphs() {
    // shared-image.pdf: two pages share one image; replace the SECRET word on page 0.
    let input = fixture("hddn/shared-image.pdf");
    let (mut e, doc) = engine(input.clone(), true);
    let t = e.extract_text(&doc, 0, ExtractOptions::default());
    let w = run(&mut e, t)
        .unwrap()
        .words
        .into_iter()
        .find(|w| w.text == "SECRET_SHARED_IMAGE")
        .unwrap();
    let b = w.bounds;
    let sel = rect(b.x - 1.0, b.y - 1.0, b.width + 2.0, b.height + 2.0);
    let r = changes(
        &mut e,
        &doc,
        Some(vec![edit("e", 0, &[sel], "SHARED IMAGE")]),
        None,
    );
    assert!(r.accepted, "{:?}", r.rejected);
    let (out, _) = export(&mut e, &doc);
    let p = probe("image.out.pdf", &out, sel, (5.0, 5.0));
    assert_eq!(
        p.images,
        probe("image.in.pdf", &input, sel, (5.0, 5.0)).images
    );
    assert!(
        p.images.iter().sum::<usize>() >= 2,
        "both pages keep the shared image"
    );
    assert_text(
        &p,
        &["KEEP_PUBLIC_SHARED_IMAGE_P1", "SHARED IMAGE"],
        &["SECRET_SHARED_IMAGE"],
    );
    let (ok, log) = fidelity_oracle(
        "paragraph-edits",
        "image",
        &input,
        &out,
        &[(0, sel.x, sel.y, sel.x + sel.width, sel.y + sel.height)],
        &["KEEP_PUBLIC_SHARED_IMAGE_P1"],
    );
    assert!(ok, "{log}");

    // unicode-glyph-widths.pdf: the line is set in an embedded TrueType program. The glyphs the
    // new text reuses must keep their outlines although the old text that used them is removed.
    let input = fixture("hddn/unicode-glyph-widths.pdf");
    let (mut e, doc) = engine(input.clone(), false);
    let t = e.extract_text(&doc, 0, ExtractOptions::default());
    let w = run(&mut e, t)
        .unwrap()
        .words
        .into_iter()
        .find(|w| w.text == "SECRET_UNICODE")
        .unwrap();
    let b = w.bounds;
    let sel = rect(b.x - 1.0, b.y - 1.0, b.width + 2.0, b.height + 2.0);
    let r = changes(&mut e, &doc, Some(vec![edit("e", 0, &[sel], "RRRR")]), None);
    assert!(r.accepted, "{:?}", r.rejected);
    assert_eq!(
        r.layouts[0]
            .runs
            .iter()
            .map(|x| x.source)
            .collect::<Vec<_>>(),
        [RunSource::Embedded]
    );
    let (out, _) = export(&mut e, &doc);
    let p = probe("dejavu.out.pdf", &out, sel, (5.0, 5.0));
    assert_text(
        &p,
        &["RRRR", "KEEP_PUBLIC_UNICODE_GLYPH_WIDTHS"],
        &["SECRET_UNICODE"],
    );
    assert!(
        p.ink > 0.02,
        "reused TrueType glyphs must still have outlines, ink {}",
        p.ink
    );
}

// ---- preserve-mode working copies (spec 8.3, NEW-79) ----------------------------------------------

/// 300 x 400 page whose one text line sits under a /Link annotation.
fn link_page() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>".to_owned(),
        {
            let c = "BT /F1 12 Tf 20 370 Td (KEEP_PUBLIC) Tj ET";
            format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len())
        },
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        "<< /Type /Annot /Subtype /Link /Rect [10 10 100 50] /Border [0 0 0] /A << /S /URI /URI (https://example.com) >> >>".to_owned(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut at = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        at.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for a in at {
        out.extend(format!("{a:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .bytes(),
    );
    out
}

/// An engine that keeps the bytes of the last working copy the paragraph applier wrote, so a
/// test can look at what export never returns.
struct Keep(Arc<Mutex<Vec<u8>>>);

impl EditApplier for Keep {
    fn apply(
        &self,
        source: &Document,
        edits: &[TextEdit],
        ctx: &EditContext<'_>,
    ) -> Result<Applied, Vec<RejectedChange>> {
        let applied = ParagraphApplier.apply(source, edits, ctx)?;
        *self.0.lock().unwrap() = applied.bytes.clone();
        Ok(applied)
    }
}

fn keeping(bytes: Vec<u8>) -> (Engine, String, Arc<Mutex<Vec<u8>>>) {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let mut e = Engine::new(Limits::default()).with_edit_applier(Box::new(Keep(kept.clone())));
    e.register_fonts(vec![sans()]).unwrap();
    let (doc, _) = e.open(bytes, None, false).unwrap();
    (e, doc, kept)
}

/// What pikepdf finds in `bytes`: annotation subtypes and link targets of page 0, the catalog
/// entries that carry forms and layers, and the number of registered (Liberation Sans) Type0
/// font objects anywhere in the file.
fn structure(name: &str, bytes: &[u8]) -> serde_json::Value {
    let path = tmp("paragraph-edits", name);
    std::fs::write(&path, bytes).unwrap();
    let code = "import sys, json, pikepdf\n\
        pdf = pikepdf.open(sys.argv[1])\n\
        a = pdf.pages[0].obj.get('/Annots', [])\n\
        fonts = sum(1 for o in pdf.objects if isinstance(o, pikepdf.Dictionary) and o.get('/Subtype') == '/Type0' and 'LiberationSans' in str(o.get('/BaseFont')))\n\
        print(json.dumps({'annots': [str(x.get('/Subtype')) for x in a],\n\
          'uri': [str(x['/A']['/URI']) for x in a if '/A' in x],\n\
          'acroform': '/AcroForm' in pdf.Root, 'ocp': '/OCProperties' in pdf.Root,\n\
          'fonts': fonts}))\n";
    let (ok, log) = python(&["-c", code, path.to_str().unwrap()]);
    assert!(ok, "{log}");
    serde_json::from_str(log.lines().rfind(|l| l.starts_with('{')).expect(&log)).unwrap()
}

fn lost(r: &ExportReport, code: LossCode) -> u32 {
    r.losses
        .iter()
        .find(|l| l.code == code)
        .map_or(0, |l| l.count)
}

/// Text of every page of `bytes`, whitespace removed, as PyMuPDF reads it (annotation
/// appearances included, hidden layers honoured).
fn visible(name: &str, bytes: &[u8]) -> String {
    flat(&probe(name, bytes, A, (0.0, 0.0)).fitz.concat())
}

#[test]
fn an_edit_keeps_the_link_in_the_working_copy_and_export_removes_it_with_a_loss() {
    let input = link_page();
    let (mut e, doc, kept) = keeping(input);
    let r = changes(
        &mut e,
        &doc,
        Some(vec![edit(
            "e",
            0,
            &[rect(15.0, 15.0, 120.0, 20.0)],
            "CHANGED",
        )]),
        None,
    );
    assert!(r.accepted, "{:?}", r.rejected);
    let working = structure("link.work.pdf", &kept.lock().unwrap());
    assert_eq!(working["annots"], serde_json::json!(["/Link"]));
    assert_eq!(working["uri"], serde_json::json!(["https://example.com"]));
    assert!(visible("link.work.vis.pdf", &kept.lock().unwrap()).contains("CHANGED"));

    let (out, report) = export(&mut e, &doc);
    let exported = structure("link.out.pdf", &out);
    assert_eq!(exported["annots"], serde_json::json!([]));
    assert_eq!(lost(&report, LossCode::LinksRemoved), 1);
    assert!(visible("link.out.vis.pdf", &out).contains("CHANGED"));
    let (ok, log) = objects_oracle(
        "paragraph-edits",
        "link.objects.pdf",
        &out,
        &["KEEP_PUBLIC"],
    );
    assert!(ok, "{log}");
}

#[test]
fn an_edit_keeps_the_form_in_the_working_copy_and_export_flattens_it() {
    let input = fixture("hddn/acroform.pdf");
    let (mut e, doc, kept) = keeping(input);
    let r = changes(
        &mut e,
        &doc,
        Some(vec![edit(
            "e",
            0,
            &[rect(70.0, 45.0, 165.0, 22.0)],
            "EDITED_PUBLIC",
        )]),
        None,
    );
    assert!(r.accepted, "{:?}", r.rejected);
    let working = structure("form.work.pdf", &kept.lock().unwrap());
    assert_eq!(working["acroform"], true);
    assert_eq!(working["annots"], serde_json::json!(["/Widget"]));

    let (out, report) = export(&mut e, &doc);
    let exported = structure("form.out.pdf", &out);
    assert_eq!(
        (exported["acroform"].clone(), exported["annots"].clone()),
        (false.into(), serde_json::json!([]))
    );
    assert!(lost(&report, LossCode::FormsFlattened) > 0);
    // The field value is still drawn, the edited text replaced the old one.
    let all = visible("form.out.vis.pdf", &out);
    assert!(
        all.contains("SECRET_FORM_VALUE") && all.contains("EDITED_PUBLIC"),
        "{all}"
    );
    assert!(!all.contains("KEEP_PUBLIC_ACROFORM"), "{all}");
}

#[test]
fn an_edit_keeps_layers_in_the_working_copy_and_export_flattens_them() {
    let input = fixture("hddn/optional-content-layer.pdf");
    let (mut e, doc, kept) = keeping(input);
    let r = changes(
        &mut e,
        &doc,
        Some(vec![edit(
            "e",
            0,
            &[rect(70.0, 45.0, 265.0, 22.0)],
            "EDITED_PUBLIC",
        )]),
        None,
    );
    assert!(r.accepted, "{:?}", r.rejected);
    assert_eq!(
        structure("ocg.work.pdf", &kept.lock().unwrap())["ocp"],
        true
    );

    let (out, report) = export(&mut e, &doc);
    assert_eq!(structure("ocg.out.pdf", &out)["ocp"], false);
    assert!(lost(&report, LossCode::LayersFlattened) > 0 || report.flattened.ocg_layers > 0);
    assert!(visible("ocg.out.vis.pdf", &out).contains("EDITED_PUBLIC"));
    let (ok, log) = objects_oracle(
        "paragraph-edits",
        "ocg.objects.pdf",
        &out,
        &["SECRET_LAYER_BODY", "SECRET_LAYER_NAME"],
    );
    assert!(ok, "{log}");
}

#[test]
fn a_registered_font_is_one_object_across_edits_and_commits() {
    let (mut e, doc, kept) = keeping(page());
    let first = [
        edit("a", 0, &[A], "ąčęėįšųūž testing"),
        edit(
            "n",
            0,
            &[rect(15.0, 188.0, 150.0, 16.0)],
            "Žaliasis rašalas",
        ),
    ];
    let r = changes(&mut e, &doc, Some(first.to_vec()), None);
    assert!(r.accepted, "{:?}", r.rejected);
    assert_eq!(structure("font1.pdf", &kept.lock().unwrap())["fonts"], 1);

    // The list replays from the source, so a later commit repeats the first two edits and adds
    // a third that needs new glyphs: the old ones keep their codes, the font stays one object.
    let mut all = first.to_vec();
    all.push(edit("a2", 0, &[A], "Ūžė ąčęėįšųūž testing"));
    let r = changes(&mut e, &doc, Some(all), None);
    assert!(r.accepted, "{:?}", r.rejected);
    assert_eq!(structure("font2.pdf", &kept.lock().unwrap())["fonts"], 1);
    let (out, _) = export(&mut e, &doc);
    assert_eq!(structure("font2.out.pdf", &out)["fonts"], 1);
    let p = probe("font2.out.probe.pdf", &out, A, (0.0, 0.0));
    assert_text(&p, &["Ūžė ąčęėįšųūž testing", "Žaliasis rašalas"], &[]);
}

#[test]
fn every_failing_edit_is_reported_and_nothing_is_applied() {
    let (mut e, doc) = engine(page(), true);
    let before = text(&mut e, &doc, 0);
    let mut bad = edit("e3", 0, &[A], "x");
    bad.font = Some("No Such Family".into());
    let r = changes(
        &mut e,
        &doc,
        Some(vec![
            edit("e1", 0, &[A], "fine"),
            edit("e2", 0, &[rect(0.0, 300.0, 50.0, 20.0)], "nothing there"),
            bad,
        ]),
        None,
    );
    assert!(!r.accepted);
    let ids: Vec<_> = r.rejected.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(ids, ["e2", "e3"], "{:?}", r.rejected);
    assert_eq!((r.revision, text(&mut e, &doc, 0)), (0, before));
}

#[test]
fn one_edit_on_a_thirty_page_document_takes_a_bounded_time() {
    let pages: Vec<&str> = (0..30).map(|_| PAGE).collect();
    let (mut e, doc) = engine(pdf_bytes(&pages, &Font::default(), 0), true);
    let t = Instant::now();
    let r = changes(
        &mut e,
        &doc,
        Some(vec![edit("e", 14, &[A], "ąčęėįšųūž testing")]),
        None,
    );
    let ms = t.elapsed().as_millis();
    assert!(r.accepted, "{:?}", r.rejected);
    eprintln!(
        "one edit on 30 pages: {ms} ms ({})",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    assert!(ms < 20_000);
    assert_eq!(r.changed_pages, [14]);
}
