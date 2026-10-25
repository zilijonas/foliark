//! Inline bold and italic runs inside a regular paragraph, as Word writes a law article's defined
//! terms (ADR 0013; NEW-113). An edit keeps every unchanged character in its own run's face and
//! new text takes the style of the run it is typed into, with the page's fonts and with a caller
//! font (the caller family's face in each run's weight and style, or a typed refusal). The
//! fixture's faces are TrueType WinAnsi subsets named as Word names Arial (regular 1 em, bold
//! 1.2 em, italic 1.1 em, size 5); one letter of a bold term comes from the bold Type0 twin. The
//! export is judged by the objects and fidelity oracles.
mod common;
use common::{fidelity_oracle, fixture, objects_oracle, run};
use foliark_engine::Engine;
use foliark_model::{
    ChangeResult, ChangeSet, ErrorCode, ExportMode, ExportPolicy, ExtractOptions, FontResolution,
    FontSource, FontStyle, Limits, Rect, StyleSpan, TextEdit, TextLook, TextPage,
};

const FILE: &str = "generated/word-inline-runs.pdf";
/// The six rows (display baselines 100 to 135), x 20 to 170.
const BLOCK: Rect = rect(15.0, 94.0, 160.0, 44.0);
/// The first three rows only: bold terms, no italic.
const TOP: Rect = rect(15.0, 94.0, 160.0, 23.0);
/// Display box `(page, x0, y0, x1, y1)` the edit may change: the rows and the four free below.
const BODY: (u32, f64, f64, f64, f64) = (0, 19.0, 95.0, 171.0, 165.0);
/// The paragraph as the page draws it: `*bold*`, `_italic_`, words joined by one space.
const MARKED: &str = "In the law a word *Gav\u{117}jas* is a body which gets data and an *Owner* is one \
                      who holds it all as set out in _Annex_ _II_ unless both the sides agree on other terms";

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

/// `MARKED` without markup: the text an editor shows.
fn plain(marked: &str) -> String {
    marked.replace(['*', '_'], "")
}

fn commit(e: &mut Engine, doc: &str, area: Rect, text: &str, font: Option<&str>) -> ChangeResult {
    let t = e.apply_changes(
        doc,
        ChangeSet {
            base_revision: e.revision(doc).unwrap(),
            text_edits: Some(vec![TextEdit {
                id: "e".into(),
                page_index: 0,
                rects: vec![area],
                text: text.into(),
                font: font.map(str::to_owned),
            }]),
            redactions: None,
        },
    );
    run(e, t).unwrap()
}

fn page(e: &mut Engine, doc: &str) -> TextPage {
    let t = e.extract_text(
        doc,
        0,
        ExtractOptions {
            chars: true,
            ..Default::default()
        },
    );
    run(e, t).unwrap()
}

/// Every word in `BODY`, in reading order, joined by one space; each glyph marked by the style
/// the page draws it in (`*bold*`, `_italic_`). The glyphs are judged one by one.
fn marked(p: &TextPage) -> String {
    let clusters = p.clusters.as_deref().unwrap();
    let text: Vec<u16> = p.text.encode_utf16().collect();
    let mut out = Vec::new();
    for w in &p.words {
        let cy = w.bounds.y + w.bounds.height / 2.0;
        if !(BODY.2..=BODY.4).contains(&cy) {
            continue;
        }
        let mut s = String::new();
        let mut open = (false, false);
        for c in clusters
            .iter()
            .filter(|c| c.start >= w.start && c.end <= w.end)
        {
            let f = &p.styles[c.style_index as usize].flags;
            if open.0 && !f.bold {
                s.push('*');
            }
            if open.1 && !f.italic {
                s.push('_');
            }
            if f.italic && !open.1 {
                s.push('_');
            }
            if f.bold && !open.0 {
                s.push('*');
            }
            open = (f.bold, f.italic);
            s.push_str(&String::from_utf16_lossy(
                &text[c.start as usize..c.end as usize],
            ));
        }
        if open.0 {
            s.push('*');
        }
        if open.1 {
            s.push('_');
        }
        out.push(s);
    }
    out.join(" ")
}

fn export(e: &mut Engine, doc: &str) -> Vec<u8> {
    let t = e.prepare_export(doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(e, t).unwrap();
    e.take_export(doc, &plan.plan_id).unwrap()
}

/// Edits `from` to `to` in the whole block with the page's fonts: the preview fits, the commit
/// has its fingerprint, and every glyph of the result is drawn in `expect`'s styles.
fn edit_keeps_runs(from: &str, to: &str, expect: &str) -> (Engine, String) {
    let (mut e, doc) = open();
    assert_eq!(marked(&page(&mut e, &doc)), MARKED);
    let text = plain(MARKED).replace(from, to);
    let p = e
        .layout_edit(&doc, 0, vec![BLOCK], &text, None, false)
        .unwrap();
    assert!(p.fits && !p.approximate, "{p:?}");
    let c = commit(&mut e, &doc, BLOCK, &text, None);
    assert!(c.accepted, "{:?}", c.rejected);
    assert_eq!(c.layouts[0].fingerprint, p.fingerprint);
    assert_eq!(marked(&page(&mut e, &doc)), expect);
    (e, doc)
}

#[test]
fn editing_a_regular_word_keeps_every_inline_bold_and_italic_glyph() {
    let (mut e, doc) = edit_keeps_runs("which", "whose", &MARKED.replace("which", "whose"));
    let input = fixture(FILE);
    let out = export(&mut e, &doc);
    let (ok, log) = objects_oracle("paragraph-runs", "input.pdf", &input, &["which"]);
    assert!(
        !ok && log.contains("which"),
        "the oracle must see the word in the input: {log}"
    );
    let (ok, log) = objects_oracle("paragraph-runs", "regular.pdf", &out, &["which"]);
    assert!(ok, "objects.py: {log}");
    let (ok, log) = fidelity_oracle(
        "paragraph-runs",
        "regular",
        &input,
        &out,
        &[BODY],
        &["KEEP_PUBLIC_RUNS", "KEEP_RUNS_LINE"],
    );
    assert!(ok, "fidelity.py: {log}");
}

#[test]
fn editing_inside_a_bold_term_keeps_it_bold() {
    // `e` is on the page in the bold face (in "Owner"): the bold font draws it.
    edit_keeps_runs(
        "Gav\u{117}jas",
        "Gav\u{117}jes",
        &MARKED.replace("Gav\u{117}jas", "Gav\u{117}jes"),
    );
}

#[test]
fn an_edit_across_a_run_boundary_keeps_each_side_word_for_word() {
    // A replaced span from inside the bold term into the regular word after it, as many words
    // as before (ADR 0013, NEW-115): each new word takes the look of the one it replaces.
    edit_keeps_runs(
        "Owner is one",
        "Owners be one",
        &MARKED.replace("*Owner* is", "*Owners* be"),
    );
}

fn liberation(face: &str, weight: u16) -> FontSource {
    FontSource {
        bytes: std::fs::read(format!(
            "{}/../foliark-text/tests/data/LiberationSans-{face}.ttf",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
        family: "Liberation Sans".into(),
        weight,
        style: FontStyle::Normal,
    }
}

#[test]
fn a_caller_font_draws_each_run_in_its_family_face_of_the_run_s_weight() {
    let (mut e, doc) = open();
    let fonts = e
        .register_fonts(vec![liberation("Regular", 400), liberation("Bold", 700)])
        .unwrap();
    let (regular, bold) = (fonts[0].id.clone(), fonts[1].id.clone());
    let top = "In the law a word *Gav\u{117}jas* is a body which gets data and an *Owner* is one \
               who holds it all";
    let text = plain(top).replace("which", "whose");
    let p = e
        .layout_edit(&doc, 0, vec![TOP], &text, Some(regular.clone()), true)
        .unwrap();
    assert!(p.fits, "{p:?}");
    // Each glyph's font: the bold face for the bold terms, the caller's regular face elsewhere.
    let text16: Vec<u16> = text.encode_utf16().collect();
    let bold_at = |w: &str| {
        let w16: Vec<u16> = w.encode_utf16().collect();
        let at = text16.windows(w16.len()).position(|x| x == w16).unwrap() as u32;
        at..at + w16.len() as u32
    };
    let (gav, owner) = (bold_at("Gav\u{117}jas"), bold_at("Owner"));
    for g in p.glyphs.as_ref().unwrap() {
        let want = if gav.contains(&g.start) || owner.contains(&g.start) {
            &bold
        } else {
            &regular
        };
        assert_eq!(&g.font, want, "{g:?}");
    }
    let c = commit(&mut e, &doc, TOP, &text, Some(&regular));
    assert!(c.accepted, "{:?}", c.rejected);
    assert_eq!(c.layouts[0].fingerprint, p.fingerprint);
    let after = marked(&page(&mut e, &doc));
    assert!(after.starts_with(&top.replace("which", "whose")), "{after}");
}

#[test]
fn a_caller_family_without_the_run_s_face_refuses() {
    // The italic reference needs Liberation Sans Italic, which is not registered: a typed
    // refusal naming the characters, never the regular face in its place.
    let (mut e, doc) = open();
    let fonts = e
        .register_fonts(vec![liberation("Regular", 400), liberation("Bold", 700)])
        .unwrap();
    let text = plain(MARKED).replace("which", "whose");
    let c = commit(&mut e, &doc, BLOCK, &text, Some(&fonts[0].id));
    assert!(!c.accepted);
    let r = &c.rejected[0];
    assert_eq!(r.code, ErrorCode::EditNoFont, "{r:?}");
    let chars = r.details.as_ref().and_then(|d| d.chars.clone()).unwrap();
    assert!(chars.contains('A') && chars.contains('x'), "{chars}");
    let err = e
        .layout_edit(
            &doc,
            0,
            vec![BLOCK],
            &text,
            Some(fonts[0].id.clone()),
            false,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::EditNoFont);
}

/// `text` marked by the looks `spans` give each range (`*bold*`, `_italic_`); the spans must tile it.
fn markup(text: &str, spans: &[StyleSpan], looks: &[TextLook]) -> String {
    let t: Vec<u16> = text.encode_utf16().collect();
    assert_eq!(spans.first().map(|s| s.start), Some(0));
    assert_eq!(spans.last().map(|s| s.end), Some(t.len() as u32));
    assert!(spans.windows(2).all(|w| w[0].end == w[1].start));
    let mut out = String::new();
    for s in spans {
        let l = &looks[s.look as usize];
        let m = if l.bold {
            "*"
        } else if l.italic {
            "_"
        } else {
            ""
        };
        out.push_str(m);
        out.push_str(&String::from_utf16_lossy(
            &t[s.start as usize..s.end as usize],
        ));
        out.push_str(m);
    }
    // The twins of one face are two looks side by side.
    out.replace("**", "").replace("__", "")
}

#[test]
fn prepare_and_preview_report_each_character_s_look() {
    let (mut e, doc) = open();
    let info = e
        .prepare_paragraph(&doc, 0, vec![BLOCK], None, true)
        .unwrap();
    let text = info.original_text.clone().unwrap();
    assert_eq!(markup(&text, &info.runs, &info.looks), MARKED);
    // Regular, bold (WinAnsi and its Type0 twin) and italic: each the page's own font, with the
    // face an editor draws it in.
    let names: Vec<String> = info
        .looks
        .iter()
        .map(|l| match &l.font {
            FontResolution::Embedded { name, .. } => name.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        names,
        ["ArialMT", "Arial-BoldMT", "Arial-BoldMT", "Arial-ItalicMT"]
    );
    assert!(info.looks.iter().all(|l| l.editor_face.is_some()));

    let edited = text
        .replace("which", "whose")
        .replace("Owner is", "Owners be");
    let p = e
        .layout_edit(&doc, 0, vec![BLOCK], &edited, None, false)
        .unwrap();
    assert_eq!(
        markup(&edited, &p.styles, &info.looks),
        MARKED
            .replace("which", "whose")
            .replace("*Owner* is", "*Owners* be")
    );
}
