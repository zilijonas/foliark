//! Edit limits from the page's own evidence (ADR 0015; NEW-129): the extended width and its line
//! budget beside rules, images, frames, columns and the page margin, and the layout order that
//! uses them only when the stable width does not fit. Pages are synthetic; Helvetica at 10 pt
//! has 5 pt glyphs (`/Widths`), the page is 300 x 400, and so is a space (no font registered).
mod common;
use common::{Font, build, run};
use foliark_engine::{Document, Engine};
use foliark_model::{
    ChangeSet, ErrorCode, FontSource, FontStyle, Limits, ParagraphInfo, Rect, TextEdit,
};
use foliark_text::Registry;
use pdf_writer::{Finish, Name, Pdf, Rect as PdfRect, Ref};

const fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// "The quick brown fox": x 20 to 115, display baseline 100.
const TEXT: &str = "BT /F1 10 Tf 20 300 Td (The quick brown fox) Tj ET\n";
const SEL: Rect = rect(15.0, 90.0, 110.0, 20.0);
const SPACE: f64 = 5.0;

fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 0.01, "{a} != {b}");
}

fn info(extra: &str) -> ParagraphInfo {
    let doc: Document = build(&[&format!("{TEXT}{extra}")], &Font::default(), 0);
    doc.prepare_paragraph(0, &[SEL], None, false, &Registry::new(), &[])
        .unwrap()
}

/// `TEXT` plus a 1 x 1 image drawn by `Do` in the unit square of `cm`.
fn image_page(cm: &str) -> Document {
    let mut pdf = Pdf::new();
    let (cat, tree, page, content, f1, im) = (
        Ref::new(1),
        Ref::new(2),
        Ref::new(3),
        Ref::new(4),
        Ref::new(5),
        Ref::new(6),
    );
    pdf.catalog(cat).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    let mut p = pdf.page(page);
    p.parent(tree)
        .media_box(PdfRect::new(0.0, 0.0, 300.0, 400.0))
        .contents(content);
    let mut r = p.resources();
    r.fonts().pair(Name(b"F1"), f1);
    r.x_objects().pair(Name(b"Im1"), im);
    r.finish();
    p.finish();
    pdf.stream(content, format!("{TEXT}q {cm} cm /Im1 Do Q").as_bytes());
    let mut f = pdf.indirect(f1).dict();
    f.pair(Name(b"Type"), Name(b"Font"));
    f.pair(Name(b"Subtype"), Name(b"Type1"));
    f.pair(Name(b"BaseFont"), Name(b"Helvetica"));
    f.pair(Name(b"FirstChar"), 0);
    f.pair(Name(b"LastChar"), 255);
    f.insert(Name(b"Widths")).array().items(vec![500; 256]);
    f.finish();
    let mut img = pdf.image_xobject(im, &[0x80]);
    img.width(1).height(1).bits_per_component(8);
    img.color_space().device_gray();
    img.finish();
    Document::open(pdf.finish(), None).unwrap()
}

#[test]
fn a_page_with_no_obstacle_extends_to_the_mirrored_margin() {
    let p = info("");
    // Stable: the page's own text extent, zero slack. Extended: the mirrored left margin.
    near(p.wrap_width, 95.0);
    near(p.max_wrap_width, 300.0 - 20.0 - 20.0);
    assert!(p.max_lines >= 1 && p.max_wrap_lines == p.max_lines);
}

#[test]
fn a_rule_or_an_image_to_the_right_caps_the_extended_width_and_not_the_stable_one() {
    let plain = info("");
    // A vertical rule at x = 200, 1 pt wide: its bounds grow by the full width.
    let rule = info("q 0 G 1 w 200 285 m 200 315 l S Q\n");
    near(rule.wrap_width, plain.wrap_width);
    near(rule.max_wrap_width, 199.0 - SPACE - 20.0);
    let image = image_page("40 0 0 30 200 285")
        .prepare_paragraph(0, &[SEL], None, false, &Registry::new(), &[])
        .unwrap();
    near(image.wrap_width, plain.wrap_width);
    near(image.max_wrap_width, 200.0 - SPACE - 20.0);
    // One that does not reach the paragraph's lines is no obstacle.
    let away = info("q 0 G 1 w 200 200 m 200 230 l S Q\n");
    near(away.max_wrap_width, plain.max_wrap_width);
}

#[test]
fn a_frame_around_the_paragraph_caps_both_directions_and_an_underline_is_no_obstacle() {
    let plain = info("");
    // A table cell: display x 10 to 160, y 80 to 120, stroked 1 pt.
    let cell = info("q 0 G 1 w 10 280 150 40 re S Q\n");
    near(cell.max_wrap_width, 161.0 - SPACE - 20.0);
    assert!(cell.max_lines < plain.max_lines, "{cell:?}");
    assert_eq!(cell.max_lines, 2, "{cell:?}");
    // The same paragraph with its underline: nothing changes.
    let underlined = info("q 0 G 0.5 w 20 299 m 115 299 l S Q\n");
    near(underlined.max_wrap_width, plain.max_wrap_width);
    assert_eq!(
        (underlined.max_lines, underlined.max_wrap_lines),
        (plain.max_lines, plain.max_wrap_lines)
    );
    // A rule under the text, clear of it, bounds the lines (stable budget too).
    let ruled = info("q 0 G 1 w 20 270 m 280 270 l S Q\n");
    assert!(ruled.max_lines < plain.max_lines, "{ruled:?}");
    assert_eq!(ruled.max_wrap_lines, ruled.max_lines);
}

#[test]
fn a_second_column_keeps_its_gutter() {
    let p = info("BT /F1 10 Tf 160 300 Td (Right column text) Tj ET\n");
    // A lone line beside a word already grows up to a space short of it (ADR 0011): the
    // extended width is no wider.
    near(p.wrap_width, 160.0 - SPACE - 20.0);
    near(p.max_wrap_width, p.wrap_width);
    // Left of the gutter, never into it.
    assert!(p.origin_x + p.max_wrap_width < 160.0);
}

// ---- the layout order, on an A4 page like the HDDN sample ---------------------------------------

/// ReportLab-like A4: Helvetica 11 pt text at x = 64, the paragraph's two lines the page's
/// longest, a closing line close below them.
const L1: &str = "Your next invoice goes to Marta Jones and the receipt will arrive soon.";
const L2: &str = "Reply to marta.jones@example.com if anything on the invoice looks wrong.";

fn a4() -> Vec<u8> {
    let mut pdf = Pdf::new();
    let (cat, tree, page, content, f1) = (
        Ref::new(1),
        Ref::new(2),
        Ref::new(3),
        Ref::new(4),
        Ref::new(5),
    );
    pdf.catalog(cat).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    let mut p = pdf.page(page);
    p.parent(tree)
        .media_box(PdfRect::new(0.0, 0.0, 595.28, 841.89))
        .contents(content);
    p.resources().fonts().pair(Name(b"F1"), f1);
    p.finish();
    let body = format!(
        "BT /F1 11 Tf 64 700 Td ({L1}) Tj ET\n\
         BT /F1 11 Tf 64 686.8 Td ({L2}) Tj ET\n\
         BT /F1 11 Tf 64 667 Td (Kind regards) Tj ET\n"
    );
    pdf.stream(content, body.as_bytes());
    let mut f = pdf.indirect(f1).dict();
    f.pair(Name(b"Type"), Name(b"Font"));
    f.pair(Name(b"Subtype"), Name(b"Type1"));
    f.pair(Name(b"BaseFont"), Name(b"Helvetica"));
    f.finish();
    pdf.finish()
}

const PARAGRAPH: Rect = rect(60.0, 135.0, 480.0, 30.0);

fn engine() -> (Engine, String) {
    let mut e = Engine::new(Limits::default());
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
    let (doc, _) = e.open(a4(), None, false).unwrap();
    (e, doc)
}

#[test]
fn seven_added_characters_fit_at_the_extended_width_and_unchanged_text_keeps_its_breaks() {
    let (mut e, doc) = engine();
    let info = e
        .prepare_paragraph(&doc, 0, vec![PARAGRAPH], None, false)
        .unwrap();
    // The stable width is where the page's longest line ends; the extended one the mirrored margin.
    near(info.origin_x, 64.0);
    near(info.max_wrap_width, 595.28 - 64.0 - 64.0);
    assert!(info.wrap_width + 20.0 < info.max_wrap_width, "{info:?}");
    assert_eq!((info.max_lines, info.max_wrap_lines), (2, 2), "{info:?}");

    let old = info.original_text.clone().unwrap();
    let same = e
        .layout_edit(&doc, 0, vec![PARAGRAPH], &old, None, false)
        .unwrap();
    assert!(same.fits);
    near(same.wrap_width, info.wrap_width);
    let kept: Vec<&str> = same.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(kept, [L1, L2]);

    // Seven characters on line 1: three lines at the stable width, two at the extended one.
    let longer = old.replacen("soon.", "soon, again.", 1);
    assert_eq!(longer.chars().count(), old.chars().count() + 7);
    let p = e
        .layout_edit(&doc, 0, vec![PARAGRAPH], &longer, None, false)
        .unwrap();
    assert!(p.fits, "{p:?}");
    assert_eq!(p.lines.len(), 2);
    near(p.wrap_width, info.max_wrap_width);
    assert!(
        p.lines
            .iter()
            .all(|l| l.width <= info.max_wrap_width + 1e-6)
    );
    let again = e
        .layout_edit(&doc, 0, vec![PARAGRAPH], &longer, None, false)
        .unwrap();
    assert_eq!(p, again, "deterministic");
    // The commit takes the preview's decision.
    let base = e.revision(&doc).unwrap();
    let t = e.apply_changes(
        &doc,
        ChangeSet {
            base_revision: base,
            text_edits: Some(vec![TextEdit {
                id: "e".into(),
                page_index: 0,
                rects: vec![PARAGRAPH],
                text: longer.clone(),
                font: None,
                reflow: None,
            }]),
            redactions: None,
        },
    );
    let r = run(&mut e, t).unwrap();
    assert!(r.accepted, "{r:?}");
    assert_eq!(r.layouts[0].fingerprint, p.fingerprint);

    // Too much for either width: refused with the extended figures.
    let (mut e, doc) = engine();
    let huge = format!("{old} {old} {old}");
    let p = e
        .layout_edit(&doc, 0, vec![PARAGRAPH], &huge, None, false)
        .unwrap();
    assert!(!p.fits);
    near(p.wrap_width, info.max_wrap_width);
    let base = e.revision(&doc).unwrap();
    let t = e.apply_changes(
        &doc,
        ChangeSet {
            base_revision: base,
            text_edits: Some(vec![TextEdit {
                id: "e".into(),
                page_index: 0,
                rects: vec![PARAGRAPH],
                text: huge,
                font: None,
                reflow: None,
            }]),
            redactions: None,
        },
    );
    let r = run(&mut e, t).unwrap();
    let d = r.rejected[0].details.as_ref().unwrap();
    assert_eq!(r.rejected[0].code, ErrorCode::EditNoFit);
    assert_eq!(
        (d.needed_lines, d.max_lines),
        (Some(p.needed_lines), Some(2))
    );
}
