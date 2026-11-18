//! Edit, export, open the export and edit again (0.3.2): a file this engine wrote is as editable
//! as its source, on the source-font path and on the registered-font path, and nothing of a
//! removed line (not even its space glyphs) stays behind. The fixture is pdf-lib shaped
//! (`generated/pdflib-type0-subset.pdf`: Type0 Identity-H subset, drawn spaces, no x/y/z).
//! Judged by PyMuPDF (real glyphs only, no synthesized spaces) and the fidelity oracle, never by
//! the engine.
mod common;
use common::{fidelity_oracle, fixture, has, python, run, tmp};
use foliark_engine::Engine;
use foliark_model::{
    ChangeSet, ExportMode, ExportPolicy, FontSource, FontStyle, Limits, Rect, Redaction, TextEdit,
};

const FILE: &str = "generated/pdflib-type0-subset.pdf";
/// The fixture's lines, top to bottom (baselines at display y 40, 70, ... 160).
const LINES: [&str; 5] = [
    "Invoice date: 2026-11-12",
    "Due on: 2026-11-26",
    "Lecture for the third-age college",
    "Kept words AB12 CD34 EF56 remain",
    "KEEP_PUBLIC_PDFLIB",
];

/// Display rect around line `i` (10 pt box glyphs from x 20, 8 pt above the baseline).
fn line(i: usize) -> Rect {
    Rect {
        x: 15.0,
        y: 29.0 + 30.0 * i as f64,
        width: 270.0,
        height: 14.0,
    }
}

fn engine(bytes: Vec<u8>) -> (Engine, String, String) {
    let mut e = Engine::new(Limits::default());
    let sans = FontSource {
        bytes: std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../foliark-text/tests/data/LiberationSans-Regular.ttf"
        ))
        .unwrap(),
        family: "Liberation Sans".into(),
        weight: 400,
        style: FontStyle::Normal,
    };
    let font = e.register_fonts(vec![sans]).unwrap().remove(0).id;
    let (doc, _) = e.open(bytes, None, false).unwrap();
    (e, doc, font)
}

fn edit(id: &str, i: usize, text: &str, font: Option<&str>) -> TextEdit {
    TextEdit {
        id: id.into(),
        page_index: 0,
        rects: vec![line(i)],
        text: text.into(),
        font: font.map(Into::into),
        reflow: None,
    }
}

fn apply(e: &mut Engine, doc: &str, edits: Vec<TextEdit>, reds: Vec<Redaction>) {
    let t = e.apply_changes(
        doc,
        ChangeSet {
            base_revision: e.revision(doc).unwrap(),
            text_edits: Some(edits),
            redactions: Some(reds),
        },
    );
    let r = run(e, t).unwrap();
    assert!(r.rejected.is_empty(), "rejected: {:?}", r.rejected);
}

fn export(e: &mut Engine, doc: &str) -> Vec<u8> {
    let t = e.prepare_export(doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(e, t).unwrap();
    e.take_export(doc, &plan.plan_id).unwrap()
}

/// What PyMuPDF reads per fixture line (glyphs ordered by x, whitespace dropped) and the number
/// of space glyphs the line draws. Spaces are never synthesized (TEXT_INHIBIT_SPACES).
fn probe(name: &str, bytes: &[u8]) -> Vec<(String, usize)> {
    let path = tmp("edit-round-trip", name);
    std::fs::write(&path, bytes).unwrap();
    let code = "import sys, json, fitz\n\
        p = fitz.open(sys.argv[1])[0]\n\
        lines = [[] for _ in range(5)]\n\
        for b in p.get_text('rawdict', flags=fitz.TEXT_INHIBIT_SPACES)['blocks']:\n\
        \x20 for l in b.get('lines', []):\n\
        \x20   for s in l['spans']:\n\
        \x20     for c in s['chars']:\n\
        \x20       i = round((c['origin'][1] - 40) / 30)\n\
        \x20       if 0 <= i < 5: lines[i].append((c['origin'][0], c['c']))\n\
        print(json.dumps([[''.join(c for _, c in sorted(l) if not c.isspace()), sum(c.isspace() for _, c in l)] for l in lines]))\n";
    let (ok, log) = python(&["-c", code, path.to_str().unwrap()]);
    assert!(ok, "{log}");
    let v: Vec<(String, usize)> =
        serde_json::from_str(log.lines().rfind(|l| l.starts_with('[')).expect(&log)).unwrap();
    v
}

fn flat(s: &str) -> String {
    s.split_whitespace().collect()
}

fn spaces(s: &str) -> usize {
    s.chars().filter(|c| *c == ' ').count()
}

#[test]
fn a_pdflib_file_edits_exports_and_edits_again_three_times_on_both_font_paths() {
    let source = fixture(FILE);
    // Each round edits the lecture line in Liberation Sans (letters the subset lacks); rounds
    // one and three also edit the date line in the file's own subset, and round three the due
    // line, whose first edit then happens on a file carrying every name this engine writes.
    let rounds = [
        (
            Some("Invoice date: 2026-12-01"),
            "Lecture for the third-age xyz",
        ),
        (None, "Lecture for the xyz college plus"),
        (
            Some("Invoice on: 2026-12-03"),
            "Lecture xyz on the zz college",
        ),
    ];
    let mut bytes = source.clone();
    let mut want: Vec<String> = LINES.iter().map(|s| s.to_string()).collect();
    for (r, (date, lecture)) in rounds.iter().enumerate() {
        let (mut e, doc, sans) = engine(bytes.clone());
        let mut edits = vec![edit("lecture", 2, lecture, Some(&sans))];
        want[2] = lecture.to_string();
        if let Some(date) = date {
            edits.push(edit("date", 0, date, None));
            want[0] = date.to_string();
        }
        if r == 2 {
            edits.push(edit("due", 1, "Due on: 2026-12-31", None));
            want[1] = "Due on: 2026-12-31".into();
        }
        apply(&mut e, &doc, edits, Vec::new());
        let out = export(&mut e, &doc);
        if r == 0 {
            // What a 0.3.1 export already carries: a source-font name and a registered font
            // whose glyph record export stripped. Later rounds must not collide with or reuse them.
            assert!(has(&out, "/FkE") && has(&out, "/FkR"));
            assert!(!has(&out, "FoliarkGlyphs"));
        }
        let read = probe(&format!("round{r}.pdf"), &out);
        for (i, w) in want.iter().enumerate() {
            assert_eq!(read[i].0, flat(w), "round {r}, line {i}: {read:?}");
        }
        for i in [0, 2] {
            assert_eq!(
                read[i].1,
                spaces(&want[i]),
                "round {r}: old spaces on line {i}"
            );
        }
        let changed: Vec<(u32, f64, f64, f64, f64)> = [0usize, 1, 2]
            .iter()
            .map(|&i| {
                let l = line(i);
                (0, l.x, l.y - 4.0, l.x + l.width, l.y + l.height + 4.0)
            })
            .collect();
        let (ok, log) = fidelity_oracle(
            "edit-round-trip",
            &format!("round{r}"),
            &bytes,
            &out,
            &changed,
            &[LINES[3], LINES[4]],
        );
        assert!(ok, "round {r}: {log}");
        bytes = out;
    }
}

#[test]
fn redacting_adjacent_words_one_by_one_leaves_no_space_glyph_between_them() {
    // Negative control: the probe counts the source's five spaces on the line.
    assert_eq!(probe("spaces-source.pdf", &fixture(FILE))[3].1, 5);
    let (mut e, doc, _) = engine(fixture(FILE));
    // "AB12" and "CD34", one region each (10 pt box glyphs, 6 pt advance).
    let word = |id: &str, x: f64| Redaction::Region {
        id: id.into(),
        page_index: 0,
        rect: Rect {
            x,
            y: 121.0,
            width: 24.0,
            height: 12.0,
        },
        fill: None,
    };
    apply(
        &mut e,
        &doc,
        Vec::new(),
        vec![word("ab", 86.0), word("cd", 116.0)],
    );
    let out = export(&mut e, &doc);
    let read = probe("spaces-redacted.pdf", &out);
    assert_eq!(read[3].0, "KeptwordsEF56remain");
    // Gone: the space between the two removed words. Kept: the four beside a kept word.
    assert_eq!(read[3].1, 4, "{read:?}");
    let (ok, log) =
        common::objects_oracle("edit-round-trip", "spaces.pdf", &out, &["AB12", "CD34"]);
    assert!(ok, "{log}");
}

#[test]
fn edited_lines_keep_no_space_glyph_of_the_text_they_replaced() {
    // Every spaced line is replaced in the file's own font, so the source's space glyph lives
    // on only through the new text; the export is then edited again.
    let rounds = [
        [
            "Invoice-date: 2026-12-01",
            "Due on 2026-12-31",
            "Lecture at the college",
            "Kept-words",
        ],
        [
            "Invoice date 2026-12-02",
            "Due 2026-12-30",
            "Lecture at a college",
            "Kept-word",
        ],
    ];
    let mut bytes = fixture(FILE);
    for (round, texts) in rounds.iter().enumerate() {
        let (mut e, doc, _) = engine(bytes);
        let edits = (texts.iter().enumerate())
            .map(|(i, t)| edit(&format!("l{i}"), i, t, None))
            .collect();
        apply(&mut e, &doc, edits, Vec::new());
        bytes = export(&mut e, &doc);
        let read = probe(&format!("spaces-edited{round}.pdf"), &bytes);
        for (i, t) in texts.iter().enumerate() {
            assert_eq!(read[i], (flat(t), spaces(t)), "round {round}, line {i}");
        }
    }
}
