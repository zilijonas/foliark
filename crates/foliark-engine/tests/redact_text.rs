//! Text redaction through the writer (spec §5.1, §5.6; NEW-33). Removed codes must be gone from
//! the extracted text of two independent extractors AND from every decoded stream and ToUnicode,
//! while every surviving glyph keeps its place (checked on the model to 0.01 pt and by the
//! fidelity oracle, tests/oracles).
mod common;
use common::{Font, pdf_bytes};
use foliark_engine::interpret::{ItemKind, SemanticPage};
use foliark_engine::redaction::plan::RedactionPlan;
use foliark_engine::redaction::text::redact_text;
use foliark_engine::writer::Writer;
use foliark_engine::writer::fonts::rebuild_to_unicode;
use foliark_engine::{Binding, Committed, Document, StoredRedaction};
use foliark_model::{ErrorCode, ExtractOptions, Limits, Quad, Rect};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("redact-text");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// The oracle venv's python (cwd = repo root); `(success, stdout+stderr)`.
fn python(args: &[&str]) -> (bool, String) {
    let py = root().join("tests/oracles/.venv/bin/python");
    assert!(
        py.exists(),
        "oracle venv missing: python3 -m venv tests/oracles/.venv && tests/oracles/.venv/bin/pip install -r tests/oracles/requirements.txt"
    );
    let out = Command::new(py)
        .args(args)
        .current_dir(root())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn stored(id: usize, rect: Rect) -> StoredRedaction {
    StoredRedaction {
        id: format!("r{id}"),
        page_index: 0,
        regions: vec![rect],
        fill: None,
        must_remove: vec![],
    }
}

fn plan_for(doc: &Document, regions: &[Rect]) -> RedactionPlan {
    let committed = Committed {
        edits: vec![],
        redactions: regions
            .iter()
            .enumerate()
            .map(|(i, r)| stored(i, *r))
            .collect(),
    };
    let binding = Binding {
        doc_id: "d".into(),
        generation: 1,
        revision: 1,
    };
    RedactionPlan::build(doc, &committed, binding).unwrap()
}

/// Every word of page 0 whose text starts with `SECRET`, as a region just inside its box.
fn secret_regions(doc: &Document) -> Vec<Rect> {
    let page = doc.extract_text(0, &ExtractOptions::default()).unwrap();
    page.words
        .iter()
        .filter(|w| w.text.starts_with("SECRET"))
        .map(|w| Rect {
            x: w.bounds.x + 0.05,
            y: w.bounds.y + 0.05,
            width: w.bounds.width - 0.1,
            height: w.bounds.height - 0.1,
        })
        .collect()
}

fn glyphs(model: &SemanticPage) -> Vec<(Option<String>, Quad, foliark_model::SourceLoc, u32)> {
    model
        .items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Glyph(g) => Some((g.unicode.clone(), g.quad_user, i.loc, g.op_glyph_index)),
            _ => None,
        })
        .collect()
}

/// The survivors of `input` against the glyphs of `output`: same text in the same order, every
/// quad corner within 0.01 pt.
fn assert_survivors_in_place(input: &Document, plan: &RedactionPlan, output: &Document) {
    let removed = plan.removed();
    let before = input.page_model(0).unwrap();
    let kept: Vec<_> = before
        .items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Glyph(g) => {
                let id = foliark_engine::text::ClusterId {
                    page: 0,
                    path: i.path.clone(),
                    loc: i.loc,
                    glyph: g.op_glyph_index,
                };
                (!removed.contains(&id)).then(|| (g.unicode.clone(), g.quad_user))
            }
            _ => None,
        })
        .collect();
    let after: Vec<_> = glyphs(&output.page_model(0).unwrap())
        .into_iter()
        .map(|(u, q, _, _)| (u, q))
        .collect();
    assert_eq!(kept.len(), after.len(), "survivor count");
    for (n, (a, b)) in kept.iter().zip(&after).enumerate() {
        assert_eq!(a.0, b.0, "survivor {n} text");
        for (p, q) in a.1.0.iter().zip(&b.1.0) {
            assert!(
                (p.x - q.x).abs() < 0.01 && (p.y - q.y).abs() < 0.01,
                "survivor {n} ({:?}) moved from ({}, {}) to ({}, {})",
                a.0,
                p.x,
                p.y,
                q.x,
                q.y
            );
        }
    }
}

/// Redact, write, reopen. Judges the output with the Python oracles: forbidden strings gone
/// from text (pypdf, PyMuPDF), decoded streams and strings; unchanged words and positions
/// outside the regions (fidelity); `preserve` strings still extractable. Returns the output.
fn redact_and_judge(
    name: &str,
    input: &[u8],
    regions: &[Rect],
    forbid: &[&str],
    preserve: &[&str],
    fidelity: bool,
) -> (Vec<u8>, RedactionPlan) {
    let doc = Document::open(input.to_vec(), None).unwrap();
    let plan = plan_for(&doc, regions);
    assert!(!plan.pages.is_empty(), "{name}: nothing planned");
    let mut w = Writer::new(doc.reader(), Limits::default());
    redact_text(&doc, &plan, &mut w).unwrap();
    let out = w.write().unwrap();
    let output = Document::open(out.clone(), None).unwrap();
    assert_survivors_in_place(&doc, &plan, &output);
    let text = output
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .text;
    for f in forbid {
        assert!(
            !text.contains(f),
            "{name}: {f} still extractable by foliark"
        );
    }

    let (inp, outp) = (
        tmp(&format!("{name}.in.pdf")),
        tmp(&format!("{name}.out.pdf")),
    );
    std::fs::write(&inp, input).unwrap();
    std::fs::write(&outp, &out).unwrap();
    let mut args = vec!["tests/oracles/objects.py", outp.to_str().unwrap()];
    for f in forbid {
        args.extend(["--forbid", f]);
    }
    let (ok, log) = python(&args);
    assert!(ok, "{name}: objects.py: {log}");
    // The oracle is not vacuous: the same forbidden strings are found in the input.
    args[1] = inp.to_str().unwrap();
    let (ok, log) = python(&args);
    assert!(!ok, "{name}: objects.py found nothing in the INPUT: {log}");

    if !fidelity {
        return (out, plan);
    }
    let rects: Vec<String> = regions
        .iter()
        .map(|r| {
            format!(
                "0:{},{},{},{}",
                r.x - 0.5,
                r.y - 0.5,
                r.x + r.width + 0.5,
                r.y + r.height + 0.5
            )
        })
        .collect();
    let mut args = vec![
        "tests/oracles/fidelity.py",
        inp.to_str().unwrap(),
        outp.to_str().unwrap(),
    ];
    for r in &rects {
        args.extend(["--changed", r]);
    }
    for p in preserve {
        args.extend(["--preserve", p]);
    }
    let (ok, log) = python(&args);
    assert!(ok, "{name}: fidelity.py: {log}");
    (out, plan)
}

fn hex(s: &str) -> String {
    s.bytes().fold(String::new(), |mut h, b| {
        let _ = write!(h, "{b:02X}");
        h
    })
}

/// Redact every `SECRET` word of `page` (Helvetica, every code 0.5 em: 5 pt at 10 pt) and
/// return the plan and the rewrite report.
fn judge_page(
    name: &str,
    page: &str,
    words: usize,
    forbid: &[&str],
    preserve: &[&str],
    fidelity: bool,
) -> (RedactionPlan, foliark_engine::redaction::text::TextReport) {
    let bytes = pdf_bytes(&[page], &Font::default(), 0);
    let doc = Document::open(bytes.clone(), None).unwrap();
    let regions = secret_regions(&doc);
    assert_eq!(regions.len(), words);
    let (_, plan) = redact_and_judge(name, &bytes, &regions, forbid, preserve, fidelity);
    let mut w = Writer::new(doc.reader(), Limits::default());
    let report = redact_text(&doc, &plan, &mut w).unwrap();
    assert_eq!(report.fonts_touched, 1);
    (plan, report)
}

#[test]
fn tj_tc_tw_hex_faux_bold_and_ocr_text_keep_their_survivors_in_place() {
    let lines = [
        "BT /F1 10 Tf 1 0 0 1 20 380 Tm (alpha SECRETONE omega) Tj ET".to_owned(),
        "BT /F1 10 Tf 1 0 0 1 20 360 Tm [(ab) -400 (SECRETTWO) -300 (cd)] TJ ET".to_owned(),
        // Tc and Tw: the spaces carry Tw, every code Tc, and the survivors must not move.
        "BT /F1 10 Tf 2 Tc 5 Tw 1 0 0 1 20 340 Tm (a SECRETTHREE b c) Tj ET".to_owned(),
        format!(
            "BT /F1 10 Tf 1 0 0 1 20 320 Tm <{}> Tj ET",
            hex("alpha SECRETFOUR end")
        ),
        // The same word drawn twice, 0.4 pt apart (faux bold): both copies go.
        "BT /F1 10 Tf 1 0 0 1 20 300 Tm (SECRETSEVEN) Tj 0.4 0 Td (SECRETSEVEN) Tj ET".to_owned(),
        // An OCR layer (render mode 3) under the page text.
        "BT /F1 10 Tf 1 0 0 1 20 280 Tm 3 Tr (SECRETEIGHT) Tj ET".to_owned(),
        "BT /F1 10 Tf 1 0 0 1 20 260 Tm (visible) Tj ET".to_owned(),
    ];
    let forbid = [
        "SECRETONE",
        "SECRETTWO",
        "SECRETTHREE",
        "SECRETFOUR",
        "SECRETSEVEN",
        "SECRETEIGHT",
    ];
    let (plan, report) = judge_page(
        "operators",
        &lines.join("\n"),
        7,
        &forbid,
        &["alpha", "omega", "visible", "end"],
        true,
    );
    assert_eq!(plan.report.clusters_removed, 9 + 9 + 11 + 10 + 2 * 11 + 11);
    // Each show operator is rewritten once: the faux-bold pair is two operators.
    assert_eq!(report.operators_rewritten, 7);
    assert_eq!(report.invisible_clusters_removed, 11);
}

#[test]
fn quote_and_double_quote_operators_keep_the_text_state_their_followers_use() {
    // ' moves to the next line before showing; " also sets Tw and Tc, which `tail` relies on.
    // (fidelity.py is skipped: pypdf places text shown by ' and " wrongly, so its chunk check
    // cannot judge these lines; the survivor check above compares every glyph to 0.01 pt.)
    let lines = [
        "BT /F1 10 Tf 12 TL 1 0 0 1 20 380 Tm (before) Tj (SECRETFIVE) ' (after) ' ET",
        "BT /F1 10 Tf 1 0 0 1 20 300 Tm 3 1 (SECRETSIX) \" ET",
        "BT /F1 10 Tf 1 0 0 1 20 250 Tm (tail end) Tj ET",
    ];
    let (plan, report) = judge_page(
        "quotes",
        &lines.join("\n"),
        2,
        &["SECRETFIVE", "SECRETSIX"],
        &["before", "after", "tail"],
        false,
    );
    assert_eq!(plan.report.clusters_removed, 10 + 9);
    assert_eq!(report.operators_rewritten, 2);
}

// ---- Identity-H, 2-byte codes, embedded TrueType --------------------------------------------------

fn assemble(objs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut o = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    o.extend_from_slice(data);
    o.extend_from_slice(b"\nendstream");
    o
}

const DESCRIPTOR: &str = "/Type /FontDescriptor /FontName /LiberationSans /Flags 32 /FontBBox [-200 -300 2000 1000] /ItalicAngle 0 /Ascent 905 /Descent -212 /CapHeight 716 /StemV 80";

fn sans() -> Vec<u8> {
    std::fs::read(root().join("crates/foliark-text/tests/data/LiberationSans-Regular.ttf")).unwrap()
}

fn gid(font: &FontRef, c: char) -> u16 {
    font.charmap().map(c).unwrap().to_u32() as u16
}

/// Type0 / Identity-H over the full Liberation Sans, every glyph 600 wide. The page content is
/// `content(code_of)`, where `code_of(text)` is the 2-byte hex of a run.
fn cid_pdf(content: impl Fn(&dyn Fn(&str) -> String) -> String) -> Vec<u8> {
    let program = sans();
    let font = FontRef::new(&program).unwrap();
    let code_of = |t: &str| -> String {
        t.chars()
            .map(|c| format!("{:04X}", gid(&font, c)))
            .collect()
    };
    let map: BTreeMap<u32, String> = (' '..='~')
        .map(|c| (u32::from(gid(&font, c)), c.to_string()))
        .collect();
    let cmap = rebuild_to_unicode(2, map).unwrap();
    assemble(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 8 0 R /Resources << /Font << /F1 4 0 R >> >> >>".to_vec(),
        b"<< /Type /Font /Subtype /Type0 /BaseFont /LiberationSans /Encoding /Identity-H /DescendantFonts [5 0 R] /ToUnicode 9 0 R >>".to_vec(),
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /LiberationSans /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 6 0 R /DW 600 /CIDToGIDMap /Identity >>".to_vec(),
        format!("<< {DESCRIPTOR} /FontFile2 7 0 R >>").into_bytes(),
        stream(&format!("/Length1 {}", program.len()), &program),
        stream("", content(&code_of).as_bytes()),
        stream("", &cmap),
    ])
}

struct Count(usize);

impl OutlinePen for Count {
    fn move_to(&mut self, _: f32, _: f32) {
        self.0 += 1;
    }
    fn line_to(&mut self, _: f32, _: f32) {
        self.0 += 1;
    }
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {
        self.0 += 1;
    }
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
        self.0 += 1;
    }
    fn close(&mut self) {}
}

fn path_len(font: &FontRef, gid: u16) -> usize {
    let mut c = Count(0);
    if let Some(o) = font.outline_glyphs().get(GlyphId::new(u32::from(gid))) {
        let _ = o.draw(
            DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
            &mut c,
        );
    }
    c.0
}

const PROBE: &str = "
import sys, fitz
d = fitz.open(sys.argv[1])
for x in range(1, d.xref_length()):
    if d.xref_get_key(x, 'Length1')[0] != 'null':
        open(sys.argv[2], 'wb').write(d.xref_stream(x))
    s = d.xref_stream(x) if d.xref_is_stream(x) else b''
    if b'beginbfchar' in s:
        print('TOUNICODE', s.decode('latin-1').replace('\\n', ' '))
";

#[test]
fn identity_h_two_byte_codes_leave_no_unicode_and_no_outline() {
    let bytes = cid_pdf(|c| {
        format!(
            "BT /F1 12 Tf 1 0 0 1 20 380 Tm <{}> Tj ET\nBT /F1 12 Tf 1 0 0 1 20 360 Tm [<{}> -250 <{}> -250 <{}>] TJ ET",
            c("Alpha SECRETCID Omega"),
            c("Beta"),
            c("SECRETCID"),
            c("Gamma"),
        )
    });
    let doc = Document::open(bytes.clone(), None).unwrap();
    let regions = secret_regions(&doc);
    assert_eq!(regions.len(), 2);
    let (out, plan) = redact_and_judge(
        "cid",
        &bytes,
        &regions,
        &["SECRETCID"],
        &["Alpha", "Omega", "Beta", "Gamma"],
        true,
    );
    assert_eq!(plan.report.clusters_removed, 18);

    // ToUnicode no longer maps the removed letters (S R I D occur nowhere else), and the
    // TrueType program lost their outlines while the survivors kept theirs.
    let (outp, ttf) = (tmp("cid.final.pdf"), tmp("cid.final.ttf"));
    std::fs::write(&outp, &out).unwrap();
    let (ok, log) = python(&["-c", PROBE, outp.to_str().unwrap(), ttf.to_str().unwrap()]);
    assert!(ok, "{log}");
    let map = log.split("TOUNICODE").nth(1).expect("a ToUnicode stream");
    // `<code> <unicode>` pairs after the codespace range.
    let pairs: Vec<&str> = map[map.find("beginbfchar").unwrap()..]
        .split('<')
        .skip(1)
        .filter_map(|t| t.split('>').next())
        .collect();
    let mapped: Vec<&str> = pairs.iter().skip(1).step_by(2).copied().collect();
    for gone in ['S', 'R', 'I', 'D'] {
        assert!(
            !mapped.contains(&format!("{:04X}", gone as u32).as_str()),
            "{gone} still mapped"
        );
    }
    assert!(
        mapped.contains(&"0041"),
        "A (Alpha) must stay mapped: {mapped:?}"
    );
    let original = sans();
    let original = FontRef::new(&original).unwrap();
    let program = std::fs::read(&ttf).unwrap();
    let emptied = FontRef::new(&program).unwrap();
    for c in "SRID".chars() {
        assert_eq!(
            path_len(&emptied, gid(&original, c)),
            0,
            "{c} kept its outline"
        );
        assert!(path_len(&original, gid(&original, c)) > 0);
    }
    for c in "AlphBetaGmO".chars() {
        assert!(
            path_len(&emptied, gid(&original, c)) > 0,
            "{c} lost its outline"
        );
    }
}

// ---- ligature, Forms, refusals --------------------------------------------------------------------

#[test]
fn a_ligature_cluster_goes_whole_and_its_neighbours_stay() {
    let font = Font {
        to_unicode: vec![(1, "fi")],
        unnamed: false,
    };
    // "a" 20..25, "b" 25..30, the fi glyph 30..35, then SECRETLIG; baseline display y 112.
    let bytes = pdf_bytes(
        &["BT /F1 10 Tf 1 0 0 1 20 288 Tm (ab\\001 SECRETLIG end) Tj ET"],
        &font,
        0,
    );
    // Only the left 40% of the ligature glyph is selected: the whole cluster is removed.
    let sliver = Rect {
        x: 30.0,
        y: 100.0,
        width: 2.0,
        height: 20.0,
    };
    let doc = Document::open(bytes.clone(), None).unwrap();
    let mut regions = secret_regions(&doc);
    regions.push(sliver);
    let (out, plan) =
        redact_and_judge("ligature", &bytes, &regions, &["SECRETLIG"], &["end"], true);
    let lig = plan.pages[0]
        .clusters
        .iter()
        .find(|c| c.multi_codepoint)
        .expect("the ligature is planned");
    assert!(lig.coverage < 0.5);
    let text = Document::open(out, None)
        .unwrap()
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .text;
    assert!(!text.contains("fi"), "{text}");
    assert!(text.starts_with("ab "), "a and b are still drawn: {text}");
}

/// A Form drawing `SECRETFORM visible`; the page draws it at the offsets in `places`.
fn form_pdf(places: &[(f64, f64)]) -> Vec<u8> {
    let page: String = places
        .iter()
        .map(|(x, y)| format!("q 1 0 0 1 {x} {y} cm /Fm Do Q\n"))
        .collect();
    let widths = vec!["500"; 256].join(" ");
    assemble(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /XObject << /Fm 5 0 R >> >> >>".to_vec(),
        stream("", page.as_bytes()),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Resources << /Font << /F1 6 0 R >> >>",
            b"BT /F1 10 Tf 20 300 Td (SECRETFORM visible) Tj ET",
        ),
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 0 /LastChar 255 /Widths [{widths}] >>").into_bytes(),
    ])
}

#[test]
fn a_form_drawn_once_is_rewritten_in_place_and_a_shared_one_is_refused_untouched() {
    let once = form_pdf(&[(0.0, 0.0)]);
    let doc = Document::open(once.clone(), None).unwrap();
    let regions = secret_regions(&doc);
    assert_eq!(regions.len(), 1);
    // fidelity.py is skipped here: pypdf reports Form text at the page origin, so its chunk
    // check cannot place Form text (the survivor check above does, to 0.01 pt).
    let (_, plan) = redact_and_judge(
        "form-once",
        &once,
        &regions,
        &["SECRETFORM"],
        &["visible"],
        false,
    );
    assert!(plan.pages[0].forms.iter().all(|f| !f.shared));

    // Drawn twice: removing from the shared stream would change the other occurrence too.
    let twice = form_pdf(&[(0.0, 0.0), (0.0, -100.0)]);
    let doc = Document::open(twice, None).unwrap();
    let first_only = secret_regions(&doc);
    let region = first_only[0];
    let plan = plan_for(&doc, &[region]);
    assert!(plan.pages[0].forms[0].shared);
    let baseline = Writer::new(doc.reader(), Limits::default())
        .write()
        .unwrap();
    let mut w = Writer::new(doc.reader(), Limits::default());
    let err = redact_text(&doc, &plan, &mut w).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        err.details.and_then(|d| d.feature).as_deref(),
        Some("redactionSharedFormText")
    );
    assert_eq!(
        w.write().unwrap(),
        baseline,
        "a refusal leaves the writer untouched"
    );
}
