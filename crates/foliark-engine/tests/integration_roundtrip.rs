//! NEW-26: the cumulative native round-trip gate. For every fixture in tests/fixtures/manifest.json:
//! strict open -> semantic model -> Foliark words -> fresh write -> strict reopen -> Foliark words
//! again -> Foliark render of both -> independent oracles (tests/integration/native_roundtrip/oracles.py:
//! objects.py, fidelity.py, PyMuPDF and pypdf words). Per-feature behaviour lives in
//! native_graphics_roundtrip.rs and native_text_roundtrip.rs; this file only proves the chain holds.
//! One row per fixture is printed in the docs/evidence/native-roundtrip.md table format.
use foliark_engine::writer::Writer;
use foliark_engine::{Document, RenderOptions};
use foliark_model::{ErrorCode, ExtractOptions, Limits, PdfError};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Refused before or at write time with a typed error; there is no output to judge. The code is
/// the contract: a fixture that starts succeeding (or fails differently) fails this gate.
const REFUSED: &[(&str, ErrorCode)] = &[
    // Hidden layers would turn visible once /OCProperties is gone (spec 8.2, flattening pending).
    (
        "hddn/optional-content-layer.pdf",
        ErrorCode::OptionalContentUnsupported,
    ),
    (
        "generated/ocg-hidden-text.pdf",
        ErrorCode::OptionalContentUnsupported,
    ),
    // Hostile inputs: the reader or the limits stop them.
    ("hddn/malformed-truncated.pdf", ErrorCode::RepairedPdf),
    ("hddn/malformed-xref.pdf", ErrorCode::RepairedPdf),
    ("hddn/oversized-image.pdf", ErrorCode::LimitExceeded),
    ("hddn/compressed-stream.pdf", ErrorCode::LimitExceeded),
    ("generated/corrupt-flate.pdf", ErrorCode::DecodeFailed),
    ("generated/forms-cyclic.pdf", ErrorCode::MalformedPdf),
    ("generated/forms-deep-40.pdf", ErrorCode::LimitExceeded),
    ("generated/many-pages-1001.pdf", ErrorCode::LimitExceeded),
];

/// Visible annotations and widgets are not allowlisted: they vanish until flattening (spec 8.2).
/// The round trip is judged on page content only (render without annotations); the oracles must
/// still see the loss, so it is never reported as a pass.
const FLATTEN_PENDING: &[&str] = &[
    "hddn/annotation-comment.pdf",
    "hddn/acroform.pdf",
    "generated/acroform-values.pdf",
    "generated/annot-appearance.pdf",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// First typed error the chain meets on a hostile or unsupported input, with the stage that raised it.
fn refusal(input: &[u8], password: Option<&str>) -> Option<(&'static str, PdfError)> {
    let doc = match Document::open(input.to_vec(), password) {
        Ok(d) => d,
        Err(e) => return Some(("open", e)),
    };
    for p in 0..doc.page_count() {
        if let Err(e) = doc.page_model(p) {
            return Some(("model", e));
        }
    }
    if let Err(e) = Writer::new(doc.reader(), Limits::default()).write() {
        return Some(("write", e));
    }
    for p in 0..doc.page_count() {
        if let Err(e) = doc.extract_text(p, &ExtractOptions::default()) {
            return Some(("extract", e));
        }
        if let Err(e) = doc.render_page(p, RenderOptions::at(1.0)) {
            return Some(("render", e));
        }
    }
    None
}

fn words(doc: &Document, page: usize) -> Vec<(String, [f64; 4])> {
    let t = doc.extract_text(page, &ExtractOptions::default()).unwrap();
    t.words
        .iter()
        .map(|w| {
            let b = w.bounds;
            (w.text.clone(), [b.x, b.y, b.x + b.width, b.y + b.height])
        })
        .collect()
}

/// Foliark words of every page must be identical in text and order, boxes within 0.5 pt.
fn compare_words(a: &Document, b: &Document) -> Result<usize, String> {
    let mut n = 0;
    for p in 0..a.page_count() {
        let (x, y) = (words(a, p), words(b, p));
        if x.len() != y.len() {
            return Err(format!("p{p}: {} words vs {}", x.len(), y.len()));
        }
        for (u, v) in x.iter().zip(&y) {
            let d =
                u.1.iter()
                    .zip(v.1)
                    .map(|(s, t)| (s - t).abs())
                    .fold(0.0, f64::max);
            if u.0 != v.0 || d > 0.5 {
                return Err(format!("p{p}: {:?} vs {:?} (box delta {d:.3})", u.0, v.0));
            }
        }
        n += x.len();
    }
    Ok(n)
}

/// Flatten-pending: every output word is an input word in the same order (the annotation or widget
/// words are the only ones allowed to disappear; the oracles must still see the loss).
fn pending_words(a: &Document, b: &Document) -> Result<usize, String> {
    let mut kept = 0;
    for p in 0..a.page_count() {
        let (x, y) = (words(a, p), words(b, p));
        let mut at = x.iter();
        for v in &y {
            let hit =
                at.any(|u| u.0 == v.0 && u.1.iter().zip(v.1).all(|(s, t)| (s - t).abs() <= 0.5));
            if !hit {
                return Err(format!(
                    "p{p}: output word {:?} is not an input word in order",
                    v.0
                ));
            }
        }
        kept += y.len();
    }
    Ok(kept)
}

/// Pixels of both Foliark renders; returns the count that differ by more than 2/255 in any channel.
fn render_diff(a: &Document, b: &Document, annotations: bool) -> Result<usize, String> {
    let opts = RenderOptions {
        annotations,
        ..RenderOptions::at(1.0)
    };
    let mut diff = 0;
    for p in 0..a.page_count() {
        let (x, y) = (
            a.render_page(p, opts.clone())
                .map_err(|e| format!("render in p{p}: {e:?}"))?,
            b.render_page(p, opts.clone())
                .map_err(|e| format!("render out p{p}: {e:?}"))?,
        );
        if x.header.width != y.header.width || x.header.height != y.header.height {
            return Err(format!("p{p}: render size differs"));
        }
        diff += x
            .data
            .iter()
            .zip(&y.data)
            .filter(|(s, t)| s.abs_diff(**t) > 2)
            .count();
    }
    Ok(diff)
}

fn sha(bytes: &[u8]) -> String {
    let mut child = Command::new("shasum")
        .args(["-a", "256"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), bytes).unwrap();
    String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()[..12].to_owned()
}

#[test]
fn native_round_trip_chain_holds_on_every_fixture() {
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root().join("tests/fixtures/manifest.json")).unwrap(),
    )
    .unwrap();
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR")).join("native-roundtrip-chain");
    std::fs::create_dir_all(&tmp).unwrap();
    let py = root().join("tests/oracles/.venv/bin/python");
    assert!(
        py.exists(),
        "oracle venv missing: python3 -m venv tests/oracles/.venv && tests/oracles/.venv/bin/pip install -r tests/oracles/requirements.txt"
    );
    let (mut failures, mut rows) = (Vec::new(), Vec::new());
    let (mut supported, mut pending, mut refused) = (0, 0, 0);
    let files = manifest["files"].as_array().unwrap();
    for f in files {
        let (id, path) = (f["id"].as_str().unwrap(), f["path"].as_str().unwrap());
        let password = f["password"].as_str();
        let file = root().join("tests/fixtures").join(path);
        let input = std::fs::read(&file).unwrap();
        let in_hash = sha(&input);
        let mut fail = |why: String| failures.push(format!("{path}: {why}"));

        if let Some((_, want)) = REFUSED.iter().find(|(p, _)| *p == path) {
            match refusal(&input, password) {
                Some((stage, e)) if e.code == *want => {
                    refused += 1;
                    rows.push(format!(
                        "| {id} | refused | {:?} at {stage} | {} | - | {in_hash} | - |",
                        e.code,
                        input.len()
                    ));
                }
                other => fail(format!("expected typed refusal {want:?}, got {other:?}")),
            }
            continue;
        }
        let doc = match Document::open(input.clone(), password) {
            Ok(d) => d,
            Err(e) => {
                fail(format!("input does not open: {e:?}"));
                continue;
            }
        };
        let pages = doc.page_count();
        let items: usize = (0..pages)
            .map(|p| {
                doc.page_model(p)
                    .map(|m| m.items.len())
                    .unwrap_or(usize::MAX)
            })
            .sum();
        let out = match Writer::new(doc.reader(), Limits::default()).write() {
            Ok(o) => o,
            Err(e) => {
                fail(format!("write refused: {e:?}"));
                continue;
            }
        };
        if Writer::new(doc.reader(), Limits::default())
            .write()
            .unwrap()
            != out
        {
            fail("writer output is not deterministic".into());
        }
        let reopened = match Document::open(out.clone(), None) {
            Ok(d) => d,
            Err(e) => {
                fail(format!("output does not reopen: {e:?}"));
                continue;
            }
        };
        if reopened.page_count() != pages {
            fail(format!("pages {} != {pages}", reopened.page_count()));
            continue;
        }
        let is_pending = FLATTEN_PENDING.contains(&path);
        let n_words = if is_pending {
            pending_words(&doc, &reopened).unwrap_or_else(|e| {
                fail(format!("foliark words: {e}"));
                0
            })
        } else {
            compare_words(&doc, &reopened).unwrap_or_else(|e| {
                fail(format!("foliark words: {e}"));
                0
            })
        };
        let px = render_diff(&doc, &reopened, !is_pending).unwrap_or_else(|e| {
            fail(format!("render: {e}"));
            usize::MAX
        });
        if px != 0 {
            fail(format!("foliark renders differ in {px} channel values"));
        }
        let out_path = tmp.join(path.replace('/', "_"));
        std::fs::write(&out_path, &out).unwrap();
        let mut cmd = Command::new(&py);
        cmd.arg("tests/integration/native_roundtrip/oracles.py")
            .arg(&file)
            .arg(&out_path)
            .current_dir(root());
        if let Some(pw) = password {
            cmd.args(["--password", pw]);
        }
        if is_pending {
            cmd.arg("--pending");
        }
        let run = cmd.output().unwrap();
        let stdout = String::from_utf8_lossy(&run.stdout);
        let Some(json) = stdout
            .rsplit_once("\n@@")
            .and_then(|(_, j)| serde_json::from_str::<serde_json::Value>(j).ok())
        else {
            fail(format!(
                "oracle crashed: {}",
                String::from_utf8_lossy(&run.stderr)
            ));
            continue;
        };
        let list = |k: &str| {
            json[k]
                .as_array()
                .map(|a| a.iter().map(|v| v.to_string()).collect::<Vec<_>>())
                .unwrap_or_default()
        };
        if is_pending {
            if json["pending_loss_seen"] != true || !list("objects").is_empty() {
                fail(format!(
                    "flatten-pending: oracles saw no loss or objects.py failed: {json}"
                ));
            }
            if out.windows(7).any(|w| w == b"/Annots" || w == b"/Widget") {
                fail("flatten-pending output kept annotations".into());
            }
            pending += 1;
            rows.push(format!("| {id} | flatten-pending | content ok, annotations dropped | {} | {} | {in_hash} | {} |", input.len(), out.len(), sha(&out)));
            continue;
        }
        let mut bad = false;
        for k in ["objects", "fidelity", "words_fitz", "words_pypdf"] {
            if !list(k).is_empty() {
                bad = true;
                fail(format!("{k}: {}", list(k).join("; ")));
            }
        }
        if !bad && px == 0 {
            supported += 1;
            rows.push(format!("| {id} | supported | pass | {} | {} | {in_hash} | {} | {pages}p {n_words}w {items}i |", input.len(), out.len(), sha(&out)));
        }
    }
    println!(
        "fixtures {} supported {supported} flatten-pending {pending} refused {refused}",
        files.len()
    );
    println!("{}", rows.join("\n"));
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!(
        supported + pending + refused,
        files.len(),
        "unclassified fixtures"
    );
}
