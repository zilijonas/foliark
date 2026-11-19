//! Aligned paragraphs (ADR 0019, 0.3.3). The fixture (`generated/pdflib-right-aligned.pdf`, pdf-lib
//! shape, a glyph is 5.87 pt) has right-aligned blocks ending at x 280, a column of right-aligned
//! amounts, a ragged-right paragraph, a lone centered title, a two-line centered block and a
//! justified pair with a first-line indent. Detection is judged against how the generator drew
//! them; edits must keep the right edge (or centre), grow toward the free side only as far as the
//! page has room, keep typed line breaks and read back exactly, round after round, as PyMuPDF
//! (not the engine) sees the written file.
mod common;
use common::{build, fixture, python, run, tmp};
use foliark_engine::Engine;
use foliark_model::{
    ChangeSet, EditPreview, ExportMode, ExportPolicy, Limits, Rect, Reflow, TextAlign, TextEdit,
};

const FILE: &str = "generated/pdflib-right-aligned.pdf";
const RIGHT: f64 = 280.0;
const GLYPH: f64 = 5.87;
/// "Ref A1" ends here, on the first block's first row.
const REF_END: f64 = 20.0 + 6.0 * GLYPH;

const fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Display rects of the fixture's blocks (baselines from the generator; glyphs 8 pt tall).
const INVOICE: Rect = rect(130.0, 30.0, 155.0, 28.0);
const PAID: Rect = rect(150.0, 75.0, 135.0, 28.0);
const LECTURE: Rect = rect(15.0, 130.0, 150.0, 28.0);
const AMOUNTS: [Rect; 3] = [
    rect(245.0, 190.0, 40.0, 12.0),
    rect(245.0, 205.0, 40.0, 12.0),
    rect(245.0, 220.0, 40.0, 12.0),
];
const JUSTIFIED: Rect = rect(15.0, 240.0, 270.0, 28.0);
const TITLE: Rect = rect(100.0, 270.0, 100.0, 12.0);
const THANKS: Rect = rect(70.0, 300.0, 160.0, 28.0);

fn open(bytes: Vec<u8>) -> (Engine, String) {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(bytes, None, false).unwrap();
    (e, doc)
}

fn align(e: &mut Engine, doc: &str, r: Rect) -> TextAlign {
    let info = e.prepare_paragraph(doc, 0, vec![r], None, false).unwrap();
    info.align.unwrap()
}

fn preview(e: &Engine, doc: &str, r: Rect, text: &str) -> EditPreview {
    e.layout_edit(doc, 0, vec![r], text, None, false).unwrap()
}

fn ends(p: &EditPreview) -> Vec<f64> {
    p.lines.iter().map(|l| l.x + l.width).collect()
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn edit(r: Rect, text: &str, reflow: Option<Reflow>) -> TextEdit {
    TextEdit {
        id: "e".into(),
        page_index: 0,
        rects: vec![r],
        text: text.into(),
        font: None,
        reflow,
    }
}

fn apply_export(bytes: Vec<u8>, edits: Vec<TextEdit>) -> Vec<u8> {
    let (mut e, doc) = open(bytes);
    let t = e.apply_changes(
        &doc,
        ChangeSet {
            base_revision: 0,
            text_edits: Some(edits),
            redactions: None,
        },
    );
    let r = run(&mut e, t).unwrap();
    assert!(r.rejected.is_empty(), "rejected: {:?}", r.rejected);
    let t = e.prepare_export(&doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(&mut e, t).unwrap();
    e.take_export(&doc, &plan.plan_id).unwrap()
}

/// PyMuPDF's rows: drawn glyphs (no spaces) by baseline, ordered by x and split where a gap is
/// wider than two glyphs, as `(baseline, x0, x1, text)`.
fn rows(name: &str, bytes: &[u8]) -> Vec<(f64, f64, f64, String)> {
    let path = tmp("paragraph-align", name);
    std::fs::write(&path, bytes).unwrap();
    let code = "import sys, json, fitz\n\
        p = fitz.open(sys.argv[1])[0]\n\
        rows = {}\n\
        for b in p.get_text('rawdict', flags=fitz.TEXT_INHIBIT_SPACES)['blocks']:\n\
        \x20 for l in b.get('lines', []):\n\
        \x20   for s in l['spans']:\n\
        \x20     for c in s['chars']:\n\
        \x20       if not c['c'].isspace(): rows.setdefault(round(c['origin'][1], 1), []).append((c['origin'][0], c['bbox'][2], c['c']))\n\
        out = []\n\
        for y, cs in sorted(rows.items()):\n\
        \x20 cs.sort()\n\
        \x20 for c in cs:\n\
        \x20   if out and out[-1][0] == y and c[0] - out[-1][2] <= 12: out[-1][2] = c[1]; out[-1][3] += c[2]\n\
        \x20   else: out.append([y, c[0], c[1], c[2]])\n\
        print(json.dumps(out))\n";
    let (ok, log) = python(&["-c", code, path.to_str().unwrap()]);
    assert!(ok, "{log}");
    serde_json::from_str(log.lines().rfind(|l| l.starts_with('[')).expect(&log)).unwrap()
}

/// PyMuPDF's row whose text contains `needle`, read word by word (a row may hold other words).
fn row_of<'a>(rows: &'a [(f64, f64, f64, String)], needle: &str) -> &'a (f64, f64, f64, String) {
    rows.iter()
        .find(|r| r.3.contains(needle))
        .unwrap_or_else(|| panic!("no row with {needle:?}: {rows:?}"))
}

#[test]
fn alignment_is_read_from_the_rows_and_stays_left_without_clear_evidence() {
    let (mut e, doc) = open(fixture(FILE));
    assert_eq!(align(&mut e, &doc, INVOICE), TextAlign::Right);
    assert_eq!(align(&mut e, &doc, PAID), TextAlign::Right);
    assert_eq!(align(&mut e, &doc, THANKS), TextAlign::Center);
    // Each amount is its own paragraph; its neighbours in the column end where it ends.
    for r in AMOUNTS {
        assert_eq!(align(&mut e, &doc, r), TextAlign::Right, "{r:?}");
    }
    // Negative controls: ragged right, a lone centered line, justified rows reaching the edge.
    assert_eq!(align(&mut e, &doc, LECTURE), TextAlign::Left);
    assert_eq!(align(&mut e, &doc, TITLE), TextAlign::Left);
    assert_eq!(align(&mut e, &doc, JUSTIFIED), TextAlign::Left);
    // A lone line that only starts at the page's left margin, with nothing ending where it ends.
    let (mut e, doc) = open(fixture("generated/pdflib-type0-subset.pdf"));
    let first = rect(15.0, 29.0, 270.0, 14.0);
    assert_eq!(align(&mut e, &doc, first), TextAlign::Left);
}

#[test]
fn an_aligned_block_reads_as_hard_lines_without_indent_spaces() {
    let (mut e, doc) = open(fixture(FILE));
    let text = |e: &mut Engine, r| {
        let i = e.prepare_paragraph(&doc, 0, vec![r], None, false).unwrap();
        i.original_text.unwrap()
    };
    // The short row first: a left-aligned reading calls the break soft (the row above ends
    // short of nothing) and indents the second row; both are wrong for a right-aligned block.
    assert_eq!(text(&mut e, PAID), "Paid: yes\nAccount ref: 2026-77");
    assert_eq!(
        text(&mut e, INVOICE),
        "Invoice date: 2026-10-12\nDue by: 2026-11-12"
    );
    assert_eq!(
        text(&mut e, THANKS),
        "Thank you for\nyour business and support"
    );
}

#[test]
fn a_right_aligned_edit_keeps_the_right_edge_and_grows_left_up_to_its_neighbour() {
    let (e, doc) = open(fixture(FILE));
    let info = {
        let mut e = Engine::new(Limits::default());
        let (d, _) = e.open(fixture(FILE), None, false).unwrap();
        e.prepare_paragraph(&d, 0, vec![INVOICE], None, false)
            .unwrap()
    };
    assert!(
        near(info.origin_x + info.wrap_width, RIGHT, 1e-3),
        "{info:?}"
    );
    // "Ref A1" on the first row bounds the room on the left, a space short of it.
    assert!(
        near(RIGHT - info.max_wrap_width, REF_END + GLYPH, 0.01),
        "{info:?}"
    );

    // Unchanged, a date a hair wider, a narrower one: each row flush right, two lines, no wrap.
    for text in [
        "Invoice date: 2026-10-12\nDue by: 2026-11-12",
        "Invoice date: 2026-12-21\nDue by: 2026-11-122",
        "Invoice date: 2026-1-2\nDue: 2026-11-12",
    ] {
        let p = preview(&e, &doc, INVOICE, text);
        assert!(p.fits && p.needed_lines == 2, "{text:?}: {p:?}");
        assert!(ends(&p).iter().all(|&x| near(x, RIGHT, 1e-3)), "{p:?}");
    }
    // A much longer first row grows leftward past the block, still on its line.
    let long = "Invoice date: 2026-10-12 to 2026\nDue by: 2026-11-12";
    let p = preview(&e, &doc, INVOICE, long);
    assert!(p.fits && p.needed_lines == 2, "{p:?}");
    assert!(
        p.lines[0].x < 130.0 && p.lines[0].x >= REF_END + GLYPH - 1e-3,
        "{p:?}"
    );
    assert!(ends(&p).iter().all(|&x| near(x, RIGHT, 1e-3)), "{p:?}");
    // Longer than the room: it wraps, every line flush right, none over "Ref A1".
    let longer = "Invoice date: 2026-10-12 to 2026-11-12\nDue by: 2026-11-12";
    let p = preview(&e, &doc, INVOICE, longer);
    assert_eq!(p.needed_lines, 3, "{p:?}");
    assert!(ends(&p).iter().all(|&x| near(x, RIGHT, 1e-3)), "{p:?}");
    assert!(
        p.lines.iter().all(|l| l.x >= REF_END + GLYPH - 1e-3),
        "{p:?}"
    );
    // Old HDDN seeds the shorter row with leading spaces; they change nothing.
    let seeded = preview(
        &e,
        &doc,
        INVOICE,
        "Invoice date: 2026-12-21\n      Due by: 2026-11-12",
    );
    let plain = preview(
        &e,
        &doc,
        INVOICE,
        "Invoice date: 2026-12-21\nDue by: 2026-11-12",
    );
    assert_eq!(ends(&seeded), ends(&plain));
    assert_eq!(
        seeded.lines.iter().map(|l| l.x).collect::<Vec<_>>(),
        plain.lines.iter().map(|l| l.x).collect::<Vec<_>>()
    );
}

#[test]
fn an_amount_in_a_column_keeps_its_right_edge_and_stops_before_its_label() {
    let (e, doc) = open(fixture(FILE));
    let p = preview(&e, &doc, AMOUNTS[0], "1250.50");
    assert!(p.fits && p.needed_lines == 1, "{p:?}");
    assert!(near(ends(&p)[0], RIGHT, 1e-3), "{p:?}");
    // "Fee" ends at 20 + 3 glyphs: the box stops a space short of it, and amounts that do not
    // fit beside it wrap there (onto the next cell's row: no fit) rather than run over it.
    let fee = 20.0 + 3.0 * GLYPH;
    let p = preview(&e, &doc, AMOUNTS[0], &["1250.50"; 6].join(" "));
    assert!(!p.fits && p.needed_lines == 2, "{p:?}");
    assert!(p.lines.iter().all(|l| l.x >= fee + GLYPH - 1e-3), "{p:?}");
    assert!(ends(&p).iter().all(|&x| near(x, RIGHT, 1e-3)), "{p:?}");
}

#[test]
fn a_centered_block_keeps_its_centre() {
    let (e, doc) = open(fixture(FILE));
    let p = preview(
        &e,
        &doc,
        THANKS,
        "Thank you all\nfor your business and support",
    );
    assert!(p.fits && p.needed_lines == 2, "{p:?}");
    for l in &p.lines {
        assert!(near(l.x + l.width / 2.0, 150.0, 1e-3), "{p:?}");
    }
}

#[test]
fn a_left_paragraph_a_hair_too_wide_keeps_its_lines_within_one_em() {
    // Helvetica at 10 pt, 5 pt glyphs: two rows from x 20, a word beside the first row at x 200
    // keeps the stable width at the rows' own (the widest row, "aaaa bbbb cccc", 70 pt).
    let content = "BT /F1 10 Tf 20 300 Td (aaaa bbbb cccc) Tj ET\n\
                   BT /F1 10 Tf 20 288 Td (dddd eeee) Tj ET\n\
                   BT /F1 10 Tf 200 300 Td (Side) Tj ET\n";
    let doc = build(&[content], &common::Font::default(), 0);
    let sel = [rect(15.0, 90.0, 80.0, 24.0)];
    let reg = foliark_text::Registry::new();
    let info = doc
        .prepare_paragraph(0, &sel, None, false, &reg, &[])
        .unwrap();
    assert_eq!(info.align, Some(TextAlign::Left));
    let base = info.wrap_width;
    let lay = |text: &str| {
        let mut e = Engine::new(Limits::default());
        let (d, _) = e
            .open(
                common::pdf_bytes(&[content], &common::Font::default(), 0),
                None,
                false,
            )
            .unwrap();
        e.layout_edit(&d, 0, sel.to_vec(), text, None, false)
            .unwrap()
    };
    // One glyph wider: the stable width would wrap "cccc" to a third line; within 1 em it fits.
    let p = lay("aaaa bbbb ccccc dddd eeee");
    assert_eq!(p.needed_lines, 2, "{p:?}");
    assert!(
        p.wrap_width > base && p.wrap_width <= base + 10.0 + 1e-6,
        "{p:?}"
    );
    assert_eq!(p.lines[0].text, "aaaa bbbb ccccc");
    // Three glyphs wider is more than 1 em: a genuine new line, at the stable width.
    let p = lay("aaaa bbbb cccccccc dddd eeee");
    assert_eq!((p.needed_lines, p.wrap_width), (3, base), "{p:?}");
    // Text that still fits is laid out exactly as before.
    let p = lay("aaaa bbbb cccc dddd eeee");
    assert_eq!((p.needed_lines, p.wrap_width), (2, base), "{p:?}");
}

#[test]
fn push_down_moves_what_lies_below_a_right_aligned_block_grown_by_lines() {
    let (e, doc) = open(fixture(FILE));
    // Four more hard lines than the room below "Paid" (the lecture paragraph at 140) holds.
    let text =
        "Paid: yes\nAccount ref: 2026-77\nDue: 2026-11-12\nDue: 2026-12-21\nDue: 2026-12-12\nPaid";
    let plain = e
        .layout_edit(&doc, 0, vec![PAID], text, None, false)
        .unwrap();
    assert!(!plain.fits, "{plain:?}");
    let p = e
        .layout_edit_with(&doc, 0, vec![PAID], text, None, false, Reflow::PushDown)
        .unwrap();
    assert!(p.fits, "{p:?}");
    let shift = p.shift.expect("a push-down");
    // The moved region holds the lecture paragraph at x 20, left of the block's own rows.
    assert!(
        shift.region.x <= 20.0 && shift.region.y <= 132.0,
        "{shift:?}"
    );
    assert!(ends(&p).iter().all(|&x| near(x, RIGHT, 1e-3)), "{p:?}");

    let out = apply_export(
        fixture(FILE),
        vec![edit(PAID, text, Some(Reflow::PushDown))],
    );
    let read = rows("push.pdf", &out);
    let lecture = row_of(&read, "Lecture");
    assert!(lecture.0 > 140.0 + 1.0, "not moved: {read:?}");
    for t in ["Due:2026-12-21", "Accountref"] {
        assert!(near(row_of(&read, t).2, RIGHT, 0.05), "{t}: {read:?}");
    }
}

#[test]
fn a_right_aligned_block_round_trips_three_times_with_its_right_edge_in_place() {
    // Wider, narrower and longer dates over three edit-export-reopen rounds, on the file the
    // round before wrote; PyMuPDF judges each export.
    let rounds = [
        "Invoice date: 2026-12-21\nDue by: 2026-11-12",
        "Invoice date: 2026-1-2\nDue by: 2026-12-210",
        "Invoice date: 2026-10-12 to 2026\nDue: 2026-1-1",
    ];
    // Negative control: the probe reads the source's rows where the generator drew them.
    let src = rows("round-src.pdf", &fixture(FILE));
    assert!(
        near(row_of(&src, "Invoice").1, RIGHT - 24.0 * GLYPH, 0.05),
        "{src:?}"
    );
    let mut bytes = fixture(FILE);
    for (r, text) in rounds.iter().enumerate() {
        let (mut e, doc) = open(bytes.clone());
        let info = e
            .prepare_paragraph(&doc, 0, vec![rect(55.0, 30.0, 230.0, 28.0)], None, false)
            .unwrap();
        // Each round finds the block it wrote right-aligned again, as two hard rows.
        assert_eq!(info.align, Some(TextAlign::Right), "round {r}: {info:?}");
        assert_eq!(info.original_text.as_deref().unwrap().lines().count(), 2);
        let sel = rect(55.0, 30.0, 230.0, 28.0);
        bytes = apply_export(bytes, vec![edit(sel, text, None)]);
        let read = rows(&format!("round{r}.pdf"), &bytes);
        for line in text.lines() {
            let want: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            let row = row_of(&read, &want);
            assert_eq!(row.3, want, "round {r}: {read:?}");
            assert!(near(row.2, RIGHT, 0.05), "round {r}, {line:?}: {read:?}");
        }
        // The rows stay two, and "Ref A1" stays where it was.
        assert!(
            near(row_of(&read, "Ref").1, 20.0, 0.01),
            "round {r}: {read:?}"
        );
        let block = read.iter().filter(|w| w.0 < 60.0 && w.1 > 60.0).count();
        assert_eq!(block, 2, "round {r}: {read:?}");
    }
}
