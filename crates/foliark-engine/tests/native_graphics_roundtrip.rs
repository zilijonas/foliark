//! Fresh native writer (NEW-24, spec §8.1): every fixture is rewritten, reopened by the
//! strict reader and judged by the independent Python oracles (tests/oracles).
use foliark_engine::Document;
use foliark_engine::writer::{NewStream, Val, Writer};
use foliark_model::{ErrorCode, Limits, ObjectId, PdfError};
use pdf_writer::{Finish, Name, Pdf, Rect, Ref};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Deliberately hostile or out-of-contract inputs: the reader or limits refuse them before a
/// writer exists (or, for the two cycle/depth Forms, the semantic checks do), so there is no
/// rewrite to judge.
const HOSTILE: &[&str] = &[
    "hddn/malformed-truncated.pdf",
    "hddn/malformed-xref.pdf",
    "hddn/oversized-image.pdf",
    "hddn/compressed-stream.pdf",
    "generated/corrupt-flate.pdf",
    "generated/forms-cyclic.pdf",
    "generated/forms-deep-40.pdf",
    "generated/many-pages-1001.pdf",
];

/// Inputs whose hidden layers would turn visible once `/OCProperties` is gone: refused with a
/// typed error until optional-content flattening lands (spec 8.2).
const REFUSED_LAYERS: &[&str] = &[
    "hddn/optional-content-layer.pdf",
    "generated/ocg-hidden-text.pdf",
];

/// Visible annotations and widgets are not allowlisted, so they are gone from the rewrite until
/// annotation flattening (spec 8.2, later task) draws them as Forms before writing. For these
/// the appearance oracle is expected to see the loss; the object oracle must still pass.
const FLATTEN_PENDING: &[&str] = &[
    "hddn/annotation-comment.pdf",
    "hddn/acroform.pdf",
    "generated/acroform-values.pdf",
    "generated/annot-appearance.pdf",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `(path, password)` for every fixture in the manifest (one key per line, no JSON crate).
fn manifest() -> Vec<(String, Option<String>)> {
    let text = std::fs::read_to_string(root().join("tests/fixtures/manifest.json")).unwrap();
    let value = |l: &str| l.split('"').nth(3).map(str::to_owned);
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for l in text.lines().map(str::trim) {
        if l.starts_with("\"path\":") {
            out.push((value(l).unwrap(), None));
        } else if l.starts_with("\"password\":") {
            out.last_mut().unwrap().1 = value(l);
        }
    }
    out
}

fn python(args: &[&str]) -> (bool, String) {
    let py = root().join("tests/oracles/.venv/bin/python");
    assert!(
        py.exists(),
        "oracle venv missing: python3 -m venv tests/oracles/.venv && pip install -r tests/oracles/requirements.txt"
    );
    let log =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("oracle-{}.log", std::process::id()));
    let file = std::fs::File::create(&log).unwrap();
    let mut child = Command::new(py)
        .args(args)
        .current_dir(root())
        .stdout(Stdio::from(file.try_clone().unwrap()))
        .stderr(Stdio::from(file))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(240);
    let ok = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status.success();
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            let _ = child.wait();
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    (ok, std::fs::read_to_string(log).unwrap_or_default())
}

fn write(doc: &Document, limits: Limits) -> Result<Vec<u8>, PdfError> {
    Writer::new(doc.reader(), limits).write()
}

#[test]
fn every_fixture_rewrites_and_passes_the_oracles() {
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR")).join("native-roundtrip");
    std::fs::create_dir_all(&tmp).unwrap();
    let (mut failures, mut passed, mut refusals, mut skipped) = (Vec::new(), 0, 0, Vec::new());
    let mut pending = 0;
    let (mut bytes_in, mut bytes_out) = (0, 0);
    for (path, password) in manifest() {
        if HOSTILE.contains(&path.as_str()) {
            skipped.push(path);
            continue;
        }
        let file = root().join("tests/fixtures").join(&path);
        let input = std::fs::read(&file).unwrap();
        let mut fail = |why: String| failures.push(format!("{path}: {why}"));
        let doc = match Document::open(input.clone(), password.as_deref()) {
            Ok(d) => d,
            Err(e) => {
                fail(format!("input does not open: {e}"));
                continue;
            }
        };
        let refused = write(&doc, Limits::default());
        if REFUSED_LAYERS.contains(&path.as_str()) {
            match refused {
                Err(e) if e.code == ErrorCode::OptionalContentUnsupported => refusals += 1,
                other => fail(format!(
                    "expected OPTIONAL_CONTENT_UNSUPPORTED, got {other:?}"
                )),
            }
            continue;
        }
        let out = match refused {
            Ok(o) => o,
            Err(e) => {
                fail(format!("write refused: {e:?}"));
                continue;
            }
        };
        let reopened = match Document::open(out.clone(), None) {
            Ok(d) => d,
            Err(e) => {
                fail(format!("output does not reopen: {e}"));
                continue;
            }
        };
        if reopened.page_count() != doc.page_count() {
            fail(format!(
                "pages {} != {}",
                reopened.page_count(),
                doc.page_count()
            ));
            continue;
        }
        if let Some(e) =
            (0..reopened.page_count()).find_map(|i| reopened.reader().page_contents(i).err())
        {
            fail(format!("output content unreadable: {e}"));
            continue;
        }
        let name = path.replace('/', "_");
        let out_path = tmp.join(&name);
        std::fs::write(&out_path, &out).unwrap();
        // The oracles compare against an unencrypted copy of the input (qpdf via pikepdf).
        let reference = match &password {
            Some(pw) => {
                let plain = tmp.join(format!("{name}.plain.pdf"));
                let (ok, log) = python(&[
                    "-c",
                    "import sys, pikepdf; pikepdf.open(sys.argv[1], password=sys.argv[2]).save(sys.argv[3], encryption=False)",
                    file.to_str().unwrap(),
                    pw,
                    plain.to_str().unwrap(),
                ]);
                assert!(ok, "{path}: cannot decrypt reference copy: {log}");
                plain
            }
            None => file.clone(),
        };
        let (o, o_log) = python(&["tests/oracles/objects.py", out_path.to_str().unwrap()]);
        let (f, f_log) = python(&[
            "tests/oracles/fidelity.py",
            reference.to_str().unwrap(),
            out_path.to_str().unwrap(),
        ]);
        if FLATTEN_PENDING.contains(&path.as_str()) {
            let bytes = String::from_utf8_lossy(&out);
            if f || ["/Annots", "/AcroForm", "/Widget"]
                .iter()
                .any(|k| bytes.contains(k))
            {
                fail(format!(
                    "flatten-pending fixture: oracle saw no loss or output kept annotations: {f_log}"
                ));
            }
            if o {
                pending += 1;
            } else {
                fail(format!("objects.py: {o_log}"));
            }
            continue;
        }
        if !o {
            fail(format!("objects.py: {o_log}"));
        }
        if !f {
            fail(format!("fidelity.py: {f_log}"));
        }
        if o && f {
            passed += 1;
            bytes_in += input.len();
            bytes_out += out.len();
        }
    }
    println!(
        "fixtures passed {passed}, typed layer refusals {refusals}, flatten-pending (objects.py only) {pending}, skipped hostile {} {skipped:?}, bytes in {bytes_in} out {bytes_out}",
        skipped.len()
    );
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// One page, content `content`, optional extra page entry written by `extra`.
fn simple_pdf(content: &[u8], font: Option<Ref>) -> Vec<u8> {
    let (cat, tree, page, stream) = (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4));
    let mut pdf = Pdf::new();
    pdf.catalog(cat).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    let mut p = pdf.page(page);
    p.parent(tree)
        .media_box(Rect::new(0.0, 0.0, 200.0, 100.0))
        .contents(stream);
    if let Some(f) = font {
        p.resources().fonts().pair(Name(b"F1"), f);
    }
    p.finish();
    pdf.stream(stream, content);
    pdf.finish()
}

fn page_text(doc: &Document, page: usize) -> String {
    let parts = doc.reader().page_contents(page).unwrap();
    parts
        .iter()
        .map(|c| String::from_utf8_lossy(&c.data).into_owned())
        .collect()
}

#[test]
fn comment_only_secret_is_dropped_and_misuse_is_typed() {
    let secret = "SECRET_ONLY_IN_COMMENT_7f3a";
    let content = format!("q % {secret}\n0.10 0.20 0.30 rg 10 10 50 20 re f\n(visible) Tj Q\n");
    let input = simple_pdf(content.as_bytes(), None);
    let doc = Document::open(input.clone(), None).unwrap();
    assert!(
        input.windows(secret.len()).any(|w| w == secret.as_bytes()),
        "control input must hold the secret"
    );
    let out = write(&doc, Limits::default()).unwrap();
    let reopened = Document::open(out.clone(), None).unwrap();
    let text = page_text(&reopened, 0);
    assert!(!text.contains(secret) && !out.windows(secret.len()).any(|w| w == secret.as_bytes()));
    assert!(
        text.contains("0.10 0.20 0.30 rg") && text.contains("(visible) Tj"),
        "{text}"
    );
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("comment-control.pdf");
    std::fs::write(&path, &out).unwrap();
    let (ok, log) = python(&[
        "tests/oracles/objects.py",
        path.to_str().unwrap(),
        "--forbid",
        secret,
    ]);
    assert!(ok, "{log}");

    // A dangling font reference is an error with the object named, not a skipped font.
    let dangling = Document::open(simple_pdf(b"q Q", Some(Ref::new(99))), None).unwrap();
    let e = write(&dangling, Limits::default()).unwrap_err();
    assert_eq!(e.code, ErrorCode::MalformedPdf);
    assert_eq!(e.details.unwrap().obj_ref.as_deref(), Some("99 0 R"));

    // The output budget is enforced while writing.
    let tiny = Limits {
        max_output_bytes: 100,
        ..Limits::default()
    };
    let e = write(&doc, tiny).unwrap_err();
    assert_eq!(e.code, ErrorCode::LimitExceeded);
    assert_eq!(e.details.unwrap().limit.as_deref(), Some("maxOutputBytes"));
}

#[test]
fn substitutions_and_appended_content_reach_the_output() {
    let content = b"q 1 0 0 rg 0 0 5 5 re f Q\n0 1 0 rg 10 10 5 5 re f\n";
    let doc = Document::open(simple_pdf(content, None), None).unwrap();
    let obj = ObjectId {
        num: 4,
        generation: 0,
    };
    let start = content.windows(4).position(|w| w == b"0 1 ").unwrap();
    let mut w = Writer::new(doc.reader(), Limits::default());
    w.replace_span(obj, start..start + 8, b"0 0 1 rg");
    w.append_page_content(0, b"0 0 0 rg 1 1 1 1 re f").unwrap();
    assert_eq!(
        w.append_page_content(5, b"").err().unwrap().code,
        ErrorCode::InvalidArgument
    );
    let out = Document::open(w.write().unwrap(), None).unwrap();
    let text = page_text(&out, 0);
    // `q` stream, original (instruction replaced), then `Q` and the appended operators on top.
    assert_eq!(
        text,
        "q\nq\n1 0 0 rg\n0 0 5 5 re\nf\nQ\n0 0 1 rg\n10 10 5 5 re\nf\nQ\n0 0 0 rg 1 1 1 1 re f"
    );

    // Whole-stream replacement goes through the same allowlist: unknown keys are dropped.
    let mut w = Writer::new(doc.reader(), Limits::default());
    w.replace_stream(
        obj,
        NewStream {
            dict: vec![("Secret".into(), Val::Str(b"nope".to_vec()))],
            data: b"0 0 1 rg".to_vec(),
        },
    );
    let bytes = w.write().unwrap();
    assert!(!bytes.windows(6).any(|w| w == b"Secret"));
    assert_eq!(
        page_text(&Document::open(bytes, None).unwrap(), 0).trim(),
        "0 0 1 rg"
    );
}
