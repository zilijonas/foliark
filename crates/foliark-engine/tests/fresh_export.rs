//! NEW-46: what the native export writes is one fresh revision holding only what the allowlist
//! reaches, the same bytes every time, within the output budget. Structure is judged on the raw
//! bytes and by the independent oracle (tests/oracles/objects.py: pikepdf + pypdf + PyMuPDF),
//! never by the engine's own reader.
mod common;
use common::{
    Font, bin_stream, fixture, has, objects_oracle, pdf_bytes, python, repo, run, stream,
    tiny_pdf_bin, tmp,
};
use foliark_engine::{Document, Engine};
use foliark_model::{
    ChangeSet, ErrorCode, ExportMode, ExportPolicy, FontSource, FontStyle, Limits, PdfError, Rect,
    Redaction, TextEdit,
};

fn engine(
    limits: Limits,
    input: &[u8],
    password: Option<&str>,
) -> Result<(Engine, String), PdfError> {
    let mut e = Engine::new(limits);
    let (doc, _) = e.open(input.to_vec(), password, false)?;
    Ok((e, doc))
}

/// One native export on `e`: the plan's length and the bytes handed over.
fn export(e: &mut Engine, doc: &str) -> Result<(u32, Vec<u8>), PdfError> {
    let t = e.prepare_export(doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(e, t)?;
    let bytes = e.take_export(doc, &plan.plan_id)?;
    assert_eq!(plan.byte_length as usize, bytes.len());
    Ok((plan.byte_length, bytes))
}

fn sanitize_only(input: &[u8], password: Option<&str>) -> Vec<u8> {
    let (mut e, doc) = engine(Limits::default(), input, password).unwrap();
    export(&mut e, &doc).unwrap().1
}

fn count(bytes: &[u8], needle: &str) -> usize {
    bytes
        .windows(needle.len())
        .filter(|w| *w == needle.as_bytes())
        .count()
}

/// The numbers of the `N G obj` headers at line starts.
fn object_heads(bytes: &[u8]) -> Vec<u32> {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .filter_map(|l| l.strip_suffix(" 0 obj")?.parse().ok())
        .collect()
}

/// A fresh, single-revision, classic-xref file whose object numbers are exactly 1..N: a gap or a
/// duplicate would be an orphan, a copied revision or an object the walk did not number.
fn assert_fresh(out: &[u8], what: &str) {
    assert!(out.starts_with(b"%PDF-1.7"), "{what}: header");
    assert_eq!(count(out, "%%EOF"), 1, "{what}: %%EOF");
    assert_eq!(count(out, "startxref"), 1, "{what}: startxref");
    assert_eq!(count(out, "\nxref\n"), 1, "{what}: classic xref table");
    for banned in [
        "/ObjStm",
        "/XRef",
        "/Prev",
        "/ID",
        "/Encrypt",
        "/Linearized",
    ] {
        assert_eq!(count(out, banned), 0, "{what}: {banned} in the output");
    }
    let mut heads = object_heads(out);
    let n = heads.len() as u32;
    heads.sort_unstable();
    assert_eq!(heads, (1..=n).collect::<Vec<_>>(), "{what}: object numbers");
    assert!(has(out, &format!("/Size {}", n + 1)), "{what}: /Size");
}

/// objects.py: single revision, no unreachable object, no dangling reference, no forbidden
/// feature, `forbid` nowhere (raw, decoded, strings, extractors).
fn assert_oracle(out: &[u8], name: &str, forbid: &[&str]) {
    let (ok, log) = objects_oracle("fresh_export", name, out, forbid);
    assert!(ok, "{name}: {log}");
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// `mustRemove` of a manifest fixture, and its password.
fn manifest(id: &str) -> (Vec<String>, Option<String>) {
    let m: serde_json::Value = serde_json::from_slice(
        &std::fs::read(repo().join("tests/fixtures/manifest.json")).unwrap(),
    )
    .unwrap();
    let f = m["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == id)
        .unwrap();
    (
        strings(&f["expected"]["mustRemove"]),
        f["password"].as_str().map(String::from),
    )
}

/// The leaky fixture saved by pikepdf with object streams and a cross-reference stream: the
/// secrets sit compressed inside an /ObjStm, so only a real decode finds them in the input.
fn objstm_input() -> Vec<u8> {
    let (src, dst) = (
        repo().join("tests/fixtures/generated/leaks-metadata-attachment-js.pdf"),
        tmp("fresh_export", "objstm.in.pdf"),
    );
    let code = "import sys, pikepdf\n\
         p = pikepdf.open(sys.argv[1])\n\
         p.save(sys.argv[2], object_stream_mode=pikepdf.ObjectStreamMode.generate, compress_streams=True)";
    let (ok, log) = python(&["-c", code, src.to_str().unwrap(), dst.to_str().unwrap()]);
    assert!(ok, "{log}");
    std::fs::read(dst).unwrap()
}

/// Objects 6 and 7 are in the xref but nothing reaches them; the catalog carries an unknown key.
fn orphan_input() -> Vec<u8> {
    tiny_pdf_bin(
        "/PieceInfo << /Vendor (PIECE_SECRET) >>",
        "",
        "BT /F1 12 Tf 20 300 Td (KEEP_PUBLIC_ORPHAN) Tj ET",
        &[
            b"<< /Note (ORPHAN_DICT_SECRET) >>".to_vec(),
            bin_stream("", b"ORPHAN_STREAM_SECRET"),
        ],
    )
}

/// One export input: what it is, the password, the secrets that must be gone from the output
/// and the public words that must stay.
struct Case {
    name: &'static str,
    bytes: Vec<u8>,
    password: Option<String>,
    secrets: Vec<String>,
    keep: &'static str,
}

fn inputs() -> Vec<Case> {
    let case = |name, bytes, password, secrets: Vec<String>, keep| Case {
        name,
        bytes,
        password,
        secrets,
        keep,
    };
    // The secret of the encrypted file is drawn text: only a redaction removes it (NEW-47).
    let (_, pw) = manifest("encrypted-aes-256");
    vec![
        case(
            "orphans",
            orphan_input(),
            None,
            ["PIECE_SECRET", "ORPHAN_DICT_SECRET", "ORPHAN_STREAM_SECRET"]
                .map(String::from)
                .to_vec(),
            "KEEP_PUBLIC_ORPHAN",
        ),
        case(
            "objstm",
            objstm_input(),
            None,
            manifest("leaks-metadata-attachment-js").0,
            "KEEP_PUBLIC_KITCHEN",
        ),
        case(
            "prior-revision",
            fixture("generated/incremental-prior-text.pdf"),
            None,
            manifest("incremental-prior-text").0,
            "KEEP_NEW_REVISION",
        ),
        case(
            "aes-256",
            fixture("generated/encrypted-aes-256.pdf"),
            pw,
            vec![],
            "KEEP_PUBLIC_ENCRYPTED",
        ),
    ]
}

#[test]
fn every_export_is_one_fresh_reachable_only_revision_with_no_source_remnants() {
    for Case {
        name,
        bytes: input,
        password,
        secrets,
        keep,
    } in inputs()
    {
        // Negative controls: the input really has the structure the writer must not carry over.
        match name {
            "objstm" => {
                assert!(count(&input, "/ObjStm") > 0 && count(&input, "/XRef") > 0);
                // The compressed secrets are invisible to a byte grep of the input.
                assert!(!has(&input, "SECRET_ATTACHMENT_BODY"));
            }
            "orphans" => assert!(has(&input, "ORPHAN_DICT_SECRET") && has(&input, "PIECE_SECRET")),
            "prior-revision" => {
                assert!(count(&input, "%%EOF") > 1 && has(&input, "SECRET_OLD_REVISION"))
            }
            _ => {}
        }
        let out = sanitize_only(&input, password.as_deref());
        assert_fresh(&out, name);
        let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        assert_oracle(&out, &format!("{name}.out.pdf"), &refs);
        // The public words survive, read by PyMuPDF rather than by Foliark.
        let p = tmp("fresh_export", &format!("{name}.words.pdf"));
        std::fs::write(&p, &out).unwrap();
        let (ok, text) = python(&[
            "-c",
            "import sys, fitz; print(''.join(pg.get_text() for pg in fitz.open(sys.argv[1])))",
            p.to_str().unwrap(),
        ]);
        assert!(ok, "{text}");
        assert!(text.contains(keep), "{name}: lost {keep}: {text}");
        // The engine's strict reader accepts what it wrote.
        Document::open(out, None).unwrap();
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

const EDIT_PAGE: &str = "q 0.8 0.9 1 rg 10 255 220 60 re f Q\n\
    BT /F1 10 Tf 20 300 Td (The quick brown fox jumps) Tj ET\n\
    BT /F1 10 Tf 20 286 Td (over the lazy dogs again.) Tj ET\n\
    BT /F1 10 Tf 20 200 Td (Keep this neighbour line) Tj ET";

fn apply(e: &mut Engine, doc: &str, edits: Vec<TextEdit>, reds: Vec<Redaction>) {
    let t = e.apply_changes(
        doc,
        ChangeSet {
            base_revision: e.revision(doc).unwrap(),
            text_edits: (!edits.is_empty()).then_some(edits),
            redactions: (!reds.is_empty()).then_some(reds),
        },
    );
    let r = run(e, t).unwrap();
    assert!(r.accepted, "{:?}", r.rejected);
}

fn paragraph_edit() -> TextEdit {
    TextEdit {
        id: "e1".into(),
        page_index: 0,
        rects: vec![Rect {
            x: 10.0,
            y: 85.0,
            width: 150.0,
            height: 37.0,
        }],
        text: "Replacement words here".into(),
        font: None,
    }
}

/// An engine with the edit page, the registered font and (optionally) the edit committed.
fn edited(limits: Limits, edit: bool) -> (Engine, String) {
    let (mut e, doc) = engine(limits, &pdf_bytes(&[EDIT_PAGE], &Font::default(), 0), None).unwrap();
    e.register_fonts(vec![sans()]).unwrap();
    if edit {
        apply(&mut e, &doc, vec![paragraph_edit()], vec![]);
    }
    (e, doc)
}

#[test]
fn edit_only_and_sanitize_only_exports_are_valid_fresh_files() {
    // Edit-only: no redaction. The replaced words are gone from every byte, the new ones read.
    let (mut e, doc) = edited(Limits::default(), true);
    let (_, out) = export(&mut e, &doc).unwrap();
    assert_fresh(&out, "edit-only");
    assert_oracle(&out, "edit-only.out.pdf", &["quick", "brown", "lazy"]);
    let p = tmp("fresh_export", "edit-only.words.pdf");
    std::fs::write(&p, &out).unwrap();
    let (ok, text) = python(&[
        "-c",
        "import sys, fitz; print(' '.join(fitz.open(sys.argv[1])[0].get_text().split()))",
        p.to_str().unwrap(),
    ]);
    assert!(
        ok && text.contains("Replacement words here") && text.contains("Keep this neighbour line"),
        "{text}"
    );

    // Sanitize-only (no edits, no redactions) is covered by every input above; the engine also
    // exports the same document unedited as a different, equally valid file.
    let (mut e, doc) = edited(Limits::default(), false);
    let (_, plain) = export(&mut e, &doc).unwrap();
    assert_fresh(&plain, "sanitize-only");
    assert_ne!(plain, out);
    assert_oracle(&plain, "sanitize-only.out.pdf", &[]);
}

#[test]
fn the_same_inputs_give_the_same_bytes_on_a_second_export_and_on_a_fresh_engine() {
    for Case {
        name,
        bytes: input,
        password,
        ..
    } in inputs()
    {
        let (mut e, doc) = engine(Limits::default(), &input, password.as_deref()).unwrap();
        let first = export(&mut e, &doc).unwrap().1;
        assert_eq!(
            first,
            export(&mut e, &doc).unwrap().1,
            "{name}: second export"
        );
        let (mut other, d2) = engine(Limits::default(), &input, password.as_deref()).unwrap();
        assert_eq!(
            first,
            export(&mut other, &d2).unwrap().1,
            "{name}: fresh engine"
        );
    }
    // With an edit, a registered font and a redaction in play.
    let run_once = || {
        let (mut e, doc) = edited(Limits::default(), false);
        let text = e.extract_text(&doc, 0, Default::default());
        let text = run(&mut e, text).unwrap().text;
        let start = text.find("neighbour").unwrap() as u32;
        let red = Redaction::Text {
            id: "r1".into(),
            page_index: 0,
            start,
            end: start + 9,
            fill: None,
        };
        apply(&mut e, &doc, vec![paragraph_edit()], vec![red]);
        export(&mut e, &doc).unwrap().1
    };
    let (a, b) = (run_once(), run_once());
    assert_eq!(a, b, "edit + redaction + registered font");
    assert_oracle(&a, "edit-redact.out.pdf", &["neighbour", "quick"]);
}

#[test]
fn the_output_budget_meters_the_held_buffer_and_the_copy_handed_to_js() {
    let (mut e, doc) = edited(Limits::default(), true);
    let (len, _) = export(&mut e, &doc).unwrap();
    let held_and_copy = 2 * u64::from(len);
    let limits = |max: u64| Limits {
        max_output_bytes: max as u32,
        ..Limits::default()
    };
    // Exactly the held buffer plus its copy fits.
    let (mut e, doc) = edited(limits(held_and_copy), true);
    assert_eq!(export(&mut e, &doc).unwrap().0, len);
    // One byte less: LIMIT_EXCEEDED naming the limit and the metered size, and no plan is held
    // (the buffer was released, not kept for a later export).
    let (mut e, doc) = edited(limits(held_and_copy - 1), true);
    let err = export(&mut e, &doc).unwrap_err();
    assert_eq!(err.code, ErrorCode::LimitExceeded);
    let d = err.details.unwrap();
    assert_eq!(d.limit.as_deref(), Some("maxOutputBytes"));
    assert_eq!(d.max, Some((held_and_copy - 1) as f64));
    assert!(d.actual.unwrap() >= held_and_copy as f64, "{:?}", d.actual);
    assert_eq!(e.stats().plan_bytes, 0);
    // A later export with the same engine still refuses the same way (nothing was half-kept).
    assert_eq!(
        export(&mut e, &doc).unwrap_err().code,
        ErrorCode::LimitExceeded
    );
}

#[test]
fn a_large_source_stream_is_refused_before_it_is_copied_into_the_output() {
    // 3 MiB of image data the writer would copy verbatim (Flate chain, kept as is). The limit
    // is below twice its size, so the walk refuses at that stream with the metered size >= 6 MiB.
    let big: Vec<u8> = (0..3u32 << 20)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect();
    let img = bin_stream(
        "/Type /XObject /Subtype /Image /Width 1000 /Height 1000 /ColorSpace /DeviceGray \
         /BitsPerComponent 8 /Filter /FlateDecode",
        &big,
    );
    let pdf = tiny_pdf_bin(
        "",
        "/Resources << /Font << /F1 5 0 R >> /XObject << /Im 6 0 R >> >>",
        "q 100 0 0 100 10 10 cm /Im Do Q",
        &[img],
    );
    let tight = Limits {
        max_output_bytes: 5 << 20,
        ..Limits::default()
    };
    let (mut e, doc) = engine(tight, &pdf, None).unwrap();
    let err = export(&mut e, &doc).unwrap_err();
    assert_eq!(err.code, ErrorCode::LimitExceeded);
    let d = err.details.unwrap();
    assert_eq!(d.limit.as_deref(), Some("maxOutputBytes"));
    assert!(d.actual.unwrap() >= f64::from(6u32 << 20), "{:?}", d.actual);
    assert_eq!(e.stats().plan_bytes, 0);
    // The same file under the default budget exports.
    let (mut e, doc) = engine(Limits::default(), &pdf, None).unwrap();
    let (_, out) = export(&mut e, &doc).unwrap();
    assert_fresh(&out, "big-stream");
}

#[test]
fn a_pages_content_streams_become_one_and_identical_objects_one() {
    // Two pages, each `/Contents` split at token boundaries over three streams, each with its own
    // copy of the same resources and Helvetica. The output keeps one content stream per page,
    // one resources dictionary and one font, numbered 1..N, and the words still read.
    const HELV: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";
    let page = |content: u32, res: u32| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources {res} 0 R /Contents [{} 0 R {} 0 R {} 0 R] >>",
            content,
            content + 1,
            content + 2
        )
    };
    let parts = |text: &str| {
        [
            stream("", "BT /F1 12 Tf"),
            stream("", &format!("20 300 Td ({text}) Tj")),
            stream("", "ET"),
        ]
    };
    // 1 catalog, 2 tree, 3 and 4 pages, 5-7 and 11-13 content parts, 8 and 9 resources, 10 and 14
    // two copies of one Helvetica.
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
        page(5, 8),
        page(11, 9),
    ];
    objs.extend(parts("FIRST_WORDS"));
    objs.extend([
        "<< /Font << /F1 10 0 R >> >>".into(),
        "<< /Font << /F1 14 0 R >> >>".into(),
        HELV.into(),
    ]);
    objs.extend(parts("SECOND_WORDS"));
    objs.push(HELV.into());
    let mut pdf = String::from("%PDF-1.7\n");
    let mut at = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        at.push(pdf.len());
        pdf += &format!("{} 0 obj\n{o}\nendobj\n", i + 1);
    }
    let xref = pdf.len();
    pdf += &format!("xref\n0 {}\n0000000000 65535 f \n", at.len() + 1);
    for h in &at {
        pdf += &format!("{h:010} 00000 n \n");
    }
    pdf += &format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        at.len() + 1
    );
    let out = sanitize_only(pdf.as_bytes(), None);
    assert_fresh(&out, "split contents");
    assert_eq!(count(&out, "\nstream\n"), 2, "one content stream per page");
    assert_eq!(count(&out, "/Helvetica"), 1, "one font");
    assert_eq!(
        object_heads(&out).len(),
        8,
        "catalog, tree, 2 pages, 2 contents, resources, font"
    );
    let p = tmp("fresh_export", "split-contents.pdf");
    std::fs::write(&p, &out).unwrap();
    let (ok, text) = python(&[
        "-c",
        "import sys, fitz; print([pg.get_text().strip() for pg in fitz.open(sys.argv[1])])",
        p.to_str().unwrap(),
    ]);
    assert!(
        ok && text.contains("['FIRST_WORDS', 'SECOND_WORDS']"),
        "{text}"
    );
}
