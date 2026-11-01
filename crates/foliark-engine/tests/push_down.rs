//! Opt-in push-down (ADR 0015 Decision B; NEW-132). The `push-*` fixtures (NEW-130) are edited
//! with `reflow: 'pushDown'` and the native exports are judged by the independent shift oracle
//! `tests/oracles/shift.py`, never by the engine: what lay in the fixture's push region is found
//! again `dy` lower, everything else where it was. Synthetic pages cover the typed refusals.
mod common;
use common::{fixture, python, run, stream, tiny_pdf, tmp};
use foliark_engine::{Document, Engine};
use foliark_model::{
    ChangeResult, ChangeSet, EditPreview, ErrorCode, ExportMode, ExportPolicy, ExtractOptions,
    FontSource, FontStyle, Limits, LinkPolicy, PageShift, Rect, Reflow, TextEdit,
};

fn engine(bytes: Vec<u8>) -> (Engine, String) {
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
    let (doc, _) = e.open(bytes, None, false).unwrap();
    (e, doc)
}

const fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
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

fn preview(e: &Engine, doc: &str, r: Rect, text: &str, reflow: Reflow) -> EditPreview {
    e.layout_edit_with(doc, 0, vec![r], text, None, false, reflow)
        .unwrap()
}

fn commit(e: &mut Engine, doc: &str, edits: Vec<TextEdit>) -> ChangeResult {
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

fn export(e: &mut Engine, doc: &str, links: LinkPolicy) -> Vec<u8> {
    let policy = ExportPolicy {
        links,
        ..ExportPolicy::default()
    };
    let t = e.prepare_export(doc, ExportMode::Native, policy);
    let plan = run(e, t).unwrap();
    e.take_export(doc, &plan.plan_id).unwrap()
}

/// `shift.py` on the fixture and the export: `(pass, log)`.
fn shift_oracle(
    name: &str,
    input: &[u8],
    output: &[u8],
    region: [f64; 4],
    dy: f64,
    changed: &[[f64; 4]],
) -> (bool, String) {
    let (i, o) = (
        tmp("push-down", &format!("{name}.in.pdf")),
        tmp("push-down", &format!("{name}.out.pdf")),
    );
    std::fs::write(&i, input).unwrap();
    std::fs::write(&o, output).unwrap();
    let r = |b: &[f64; 4]| format!("0:{},{},{},{}", b[0], b[1], b[2], b[3]);
    let (region, dy) = (r(&region), dy.to_string());
    let changed: Vec<String> = changed.iter().map(r).collect();
    let mut args = vec![
        "tests/oracles/shift.py",
        i.to_str().unwrap(),
        o.to_str().unwrap(),
        "--region",
        &region,
        "--dy",
        &dy,
    ];
    for c in &changed {
        args.extend(["--changed", c]);
    }
    python(&args)
}

/// The fixture's `PUSH` region from the manifest (display space).
fn push_region(id: &str) -> [f64; 4] {
    let m: serde_json::Value = serde_json::from_slice(&fixture("manifest.json")).unwrap();
    let f = m["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == id)
        .unwrap();
    let r = &f["expected"]["pushDown"]["region"];
    std::array::from_fn(|k| r[k].as_f64().unwrap())
}

struct Case {
    id: &'static str,
    sel: Rect,
    text: &'static str,
    links: LinkPolicy,
}

/// Preview, commit and export `case` with push-down; the export is judged by the shift oracle
/// against the fixture's own push region, reopened strictly, and made twice to equal bytes.
fn pushed(case: &Case) -> (PageShift, Vec<u8>) {
    let input = fixture(&format!("generated/{}.pdf", case.id));
    let (mut e, doc) = engine(input.clone());
    let none = preview(&e, &doc, case.sel, case.text, Reflow::None);
    assert!(!none.fits, "{}: needs push-down", case.id);
    let p = preview(&e, &doc, case.sel, case.text, Reflow::PushDown);
    assert!(p.fits && p.reflow_blocked.is_none(), "{}: {p:?}", case.id);
    let shift = p.shift.expect("a shift");
    assert_ne!(p.fingerprint, none.fingerprint);
    let r = commit(
        &mut e,
        &doc,
        vec![edit(case.sel, case.text, Some(Reflow::PushDown))],
    );
    assert!(r.accepted, "{}: {:?}", case.id, r.rejected);
    assert_eq!(r.layouts[0].fingerprint, p.fingerprint);
    let out = export(&mut e, &doc, case.links);
    Document::open(out.clone(), None).expect("strict reopen");
    let (mut e2, doc2) = engine(input.clone());
    commit(
        &mut e2,
        &doc2,
        vec![edit(case.sel, case.text, Some(Reflow::PushDown))],
    );
    assert_eq!(export(&mut e2, &doc2, case.links), out, "deterministic");
    // The edit rewrites the paragraph's box down to its new last line.
    let s = case.sel;
    let last = p.lines.last().unwrap().baseline + 4.0;
    let changed = [s.x, s.y - 4.0, s.x + s.width, last.max(s.y + s.height)];
    let (ok, log) = shift_oracle(
        case.id,
        &input,
        &out,
        push_region(case.id),
        shift.dy,
        &[changed],
    );
    assert!(ok, "{}: shift.py: {log}", case.id);
    (shift, out)
}

const LETTER: &str = "Thank you for contacting us. We updated the details on your account:\n\
    It takes effect from the first of next month.\nPlease keep this letter for your records.";

#[test]
fn letter_pushes_the_details_and_signature_and_keeps_the_footer() {
    let (shift, _) = pushed(&Case {
        id: "push-letter",
        sel: rect(60.0, 190.0, 480.0, 14.0),
        text: LETTER,
        links: LinkPolicy::Remove,
    });
    // One line more than fits at the extended width: one line step (13.2 for a lone 11 pt line).
    assert!((shift.dy - 13.2).abs() < 1e-9, "{shift:?}");
}

#[test]
fn two_column_moves_only_its_own_column() {
    let (shift, _) = pushed(&Case {
        id: "push-two-column",
        sel: rect(60.0, 100.0, 230.0, 40.0),
        text: "Revenue grew in every region this\nquarter, led by the north, and the\n\
               order book is full into spring.\nIt takes effect from the first of\n\
               next month.",
        links: LinkPolicy::Remove,
    });
    assert!(shift.region.x + shift.region.width < 302.0, "{shift:?}");
}

#[test]
fn link_below_moves_with_its_text_and_is_kept() {
    let (_, out) = pushed(&Case {
        id: "push-link-below",
        sel: rect(60.0, 100.0, 480.0, 40.0),
        // One soft-wrapped paragraph, so the export finds the edit's text and keeps the links.
        text: "Your plan renews on the first of the month and the new price applies from that \
               date. It takes effect from the first of next month, and every invoice after that \
               shows the new amount next to the old one. Please keep this notice with your \
               contract papers, and write to us if anything in it looks wrong to you. Thank you \
               for staying with us. Our team reads every message and answers within two working \
               days, also during the holidays.",
        links: LinkPolicy::Keep,
    });
    assert!(common::has(&out, "example.com/help"));
}

#[test]
fn invoice_table_moves_as_a_unit() {
    pushed(&Case {
        id: "push-invoice-table",
        sel: rect(60.0, 95.0, 480.0, 30.0),
        text: "Payment is due within 14 days of the invoice date. Late payments\n\
               may be charged interest as set out in the terms.\n\
               It takes effect from the first of next month.\nPlease quote the invoice number.",
        links: LinkPolicy::Remove,
    });
}

#[test]
fn without_push_down_the_edit_still_refuses_and_reports_the_budget() {
    let (mut e, doc) = engine(fixture("generated/push-letter.pdf"));
    let sel = rect(60.0, 190.0, 480.0, 14.0);
    let info = e
        .prepare_paragraph(&doc, 0, vec![sel], None, false)
        .unwrap();
    let push = info.push_down.expect("room below");
    assert!(push.max_lines > info.max_wrap_lines);
    let r = commit(&mut e, &doc, vec![edit(sel, LETTER, None)]);
    assert!(!r.accepted);
    assert_eq!(r.rejected[0].code, ErrorCode::EditNoFit);
    assert_eq!(r.rejected[0].details.as_ref().unwrap().feature, None);
}

// ---- synthetic refusals: 300 x 400 pages, Helvetica 10 pt with 5 pt glyphs ----------------------

/// "Alpha beta gamma delta" at display baseline 40, two lines below it, a footer far below.
const PARA: &str = "BT /F1 10 Tf 20 360 Td (Alpha beta gamma delta) Tj ET\n";
const BELOW: &str =
    "BT /F1 10 Tf 20 330 Td (Below one) Tj ET\nBT /F1 10 Tf 20 316 Td (Below two) Tj ET\n";
const FOOTER: &str = "BT /F1 8 Tf 20 30 Td (Footer) Tj ET\n";
const SEL: Rect = rect(15.0, 30.0, 270.0, 15.0);
const THREE: &str = "Alpha beta gamma delta\nsecond line\nthird line";

fn page(content: &str, objs: &[String], extra: &str) -> Vec<u8> {
    let font = "/F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 0 /LastChar 255 /Widths [256 0 R] >>";
    let _ = font;
    tiny_pdf(
        "",
        &format!("/Resources << /Font << /F1 5 0 R >> {extra} >>"),
        content,
        objs,
    )
}

/// The push-down refusal feature of editing `SEL` to three lines.
fn refusal(bytes: Vec<u8>) -> Option<String> {
    let (mut e, doc) = engine(bytes);
    let p = preview(&e, &doc, SEL, THREE, Reflow::PushDown);
    assert!(!p.fits && p.shift.is_none(), "{p:?}");
    let r = commit(&mut e, &doc, vec![edit(SEL, THREE, Some(Reflow::PushDown))]);
    assert!(!r.accepted);
    assert_eq!(r.rejected[0].code, ErrorCode::EditNoFit);
    let feature = r.rejected[0].details.as_ref().unwrap().feature.clone();
    assert_eq!(feature, p.reflow_blocked);
    feature
}

#[test]
fn a_plain_page_pushes_and_its_footer_stays() {
    let bytes = page(&format!("{PARA}{BELOW}{FOOTER}"), &[], "");
    let (mut e, doc) = engine(bytes.clone());
    let p = preview(&e, &doc, SEL, THREE, Reflow::PushDown);
    let shift = p.shift.expect("pushes");
    assert!((shift.dy - 12.0).abs() < 1e-9, "{shift:?}");
    assert!(commit(&mut e, &doc, vec![edit(SEL, THREE, Some(Reflow::PushDown))]).accepted);
    let out = export(&mut e, &doc, LinkPolicy::Remove);
    let (ok, log) = shift_oracle(
        "plain",
        &bytes,
        &out,
        [15.0, 55.0, 290.0, 100.0],
        shift.dy,
        &[[15.0, 25.0, 290.0, 70.0]],
    );
    assert!(ok, "{log}");
}

#[test]
fn a_frame_around_paragraph_and_content_stays_and_bounds_the_push() {
    // A frame from display y 10 to 150 holds the paragraph and the lines below.
    let frame = "10 250 280 140 re S\n";
    let bytes = page(&format!("{frame}{PARA}{BELOW}{FOOTER}"), &[], "");
    let (mut e, doc) = engine(bytes);
    let p = preview(&e, &doc, SEL, THREE, Reflow::PushDown);
    assert!(p.fits, "{p:?}");
    let info = e
        .prepare_paragraph(&doc, 0, vec![SEL], None, false)
        .unwrap();
    let push = info.push_down.unwrap();
    // "Below two" ends near display 86; the frame's bottom (150) bounds the push.
    let dy = f64::from(push.max_lines - info.max_wrap_lines) * info.line_step;
    let room = 150.5 - (push.region.y + push.region.height + dy);
    assert!(room >= 0.0 && room < info.line_step, "{push:?}");
    assert!(commit(&mut e, &doc, vec![edit(SEL, THREE, Some(Reflow::PushDown))]).accepted);
    let out = export(&mut e, &doc, LinkPolicy::Remove);
    let moved = Document::open(out, None).unwrap();
    let text = moved.extract_text(0, &ExtractOptions::default()).unwrap();
    let below = text.words.iter().find(|w| w.text == "Below").unwrap();
    let footer = text.words.iter().find(|w| w.text == "Footer").unwrap();
    assert!(
        (below.bounds.y + below.bounds.height - (70.0 + 12.0)).abs() < 3.5,
        "{below:?}"
    );
    assert!(footer.bounds.y > 360.0, "{footer:?}");
}

#[test]
fn a_rule_through_the_cut_refuses_straddle() {
    // A vertical rule under the paragraph's words from display 25 to 100.
    let rule = "100 375 m 100 300 l S\n";
    let f = refusal(page(&format!("{PARA}{rule}{BELOW}{FOOTER}"), &[], ""));
    assert_eq!(f.as_deref(), Some("reflowStraddle"));
}

#[test]
fn one_text_object_with_moving_and_fixed_lines_refuses() {
    let shared = "BT /F1 10 Tf 20 330 Td (Below one) Tj 0 -300 Td (Footer) Tj ET\n";
    let f = refusal(page(&format!("{PARA}{shared}"), &[], ""));
    assert_eq!(f.as_deref(), Some("reflowSharedBlock"));
}

#[test]
fn a_clip_that_is_not_a_rectangle_refuses() {
    let clip = "20 395 m 290 395 l 150 200 l h W n\n";
    let f = refusal(page(&format!("{PARA}{clip}{BELOW}{FOOTER}"), &[], ""));
    assert_eq!(f.as_deref(), Some("reflowClipped"));
}

#[test]
fn a_form_with_moving_and_fixed_text_refuses() {
    let form = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Resources << /Font << /F1 5 0 R >> >>",
        &format!("{BELOW}{FOOTER}"),
    );
    let f = refusal(page(
        &format!("{PARA}/Fm Do\n"),
        &[form],
        "/XObject << /Fm 6 0 R >>",
    ));
    assert_eq!(f.as_deref(), Some("reflowSplitsForm"));
}

#[test]
fn a_note_over_moving_content_refuses() {
    let note = "<< /Type /Annot /Subtype /Text /Rect [20 320 40 340] /Contents (n) >>".to_owned();
    let bytes = tiny_pdf(
        "",
        "/Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R]",
        &format!("{PARA}{BELOW}{FOOTER}"),
        &[note],
    );
    assert_eq!(refusal(bytes).as_deref(), Some("reflowAnnotation"));
}

#[test]
fn hddn_sample_paragraph_gains_two_lines_with_push_down() {
    // ADR 0015's HDDN sample: the two-line Marta Jones paragraph, with sign-off and signature below.
    let (mut e, doc) = engine(fixture("generated/push-letter.pdf"));
    let sel = rect(60.0, 282.0, 480.0, 34.0);
    let info = e
        .prepare_paragraph(&doc, 0, vec![sel], None, false)
        .unwrap();
    let text = format!(
        "{}\nIt takes effect from the first of next month.\nPlease keep this letter.",
        info.original_text.as_deref().unwrap()
    );
    let none = preview(&e, &doc, sel, &text, Reflow::None);
    assert!(!none.fits);
    let p = preview(&e, &doc, sel, &text, Reflow::PushDown);
    let shift = p.shift.expect("pushes");
    assert!(p.fits && p.needed_lines == 4, "{p:?}");
    let pushed = f64::from(p.needed_lines - info.max_wrap_lines) * info.line_step;
    assert!((shift.dy - pushed).abs() < 1e-9, "{shift:?} {info:?}");
    eprintln!(
        "HDDN sample: maxWrapLines {} needed {} dy {} region {:?}",
        info.max_wrap_lines, p.needed_lines, shift.dy, shift.region
    );
    let r = commit(&mut e, &doc, vec![edit(sel, &text, Some(Reflow::PushDown))]);
    assert!(r.accepted, "{:?}", r.rejected);
}
