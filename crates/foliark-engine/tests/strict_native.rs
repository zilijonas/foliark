//! NEW-47: the strict native safety proof. Every fixture in tests/fixtures/manifest.json (the 35 inputs
//! plus the feature controls) goes through the public engine twice: native export with a redaction over
//! each seeded secret, and sanitize-only export. Supported cases are judged by the independent oracles in
//! tests/integration/strict_native/check.py (objects.py, fidelity.py, PyMuPDF); refused cases must stop
//! with their exact typed code and hand over no bytes. No skips: every manifest entry is a row.
//! `cargo test -p foliark-engine --test strict_native -- --nocapture` prints the evidence table.
mod common;
use common::{load_cjk_pack, python, repo, run, tmp};
use foliark_engine::Engine;
use foliark_model::{
    ChangeSet, ErrorCode, ExportMode, ExportPolicy, ExportReport, ExtractOptions, Limits, LossCode,
    PdfError, Rect, Redaction, SanitizedItem,
};
use serde_json::{Value, json};

/// Refused with a typed code and no bytes. The code is the contract: a fixture that starts
/// succeeding, or fails differently, fails this gate.
const REFUSED: &[(&str, ErrorCode)] = &[
    ("malformed-truncated", ErrorCode::RepairedPdf),
    ("malformed-xref", ErrorCode::RepairedPdf),
    ("oversized-image", ErrorCode::LimitExceeded),
    ("compressed-stream", ErrorCode::LimitExceeded),
    ("corrupt-flate", ErrorCode::DecodeFailed),
    ("forms-cyclic", ErrorCode::MalformedPdf),
    ("forms-deep-40", ErrorCode::LimitExceeded),
    ("many-pages-1001", ErrorCode::LimitExceeded),
];

fn sha(bytes: &[u8]) -> String {
    let mut child = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), bytes).unwrap();
    String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()[..12].to_owned()
}

struct Case {
    id: String,
    path: String,
    password: Option<String>,
    features: Vec<String>,
    secrets: Vec<String>,
    keep: Vec<String>,
    /// (kind, page, display-space rect)
    regions: Vec<(String, u32, Rect)>,
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn cases() -> Vec<Case> {
    let m: Value = serde_json::from_slice(
        &std::fs::read(repo().join("tests/fixtures/manifest.json")).unwrap(),
    )
    .unwrap();
    m["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let e = &f["expected"];
            let mut secrets = strings(&e["mustRemove"]);
            secrets.sort();
            secrets.dedup();
            let mut regions = Vec::new();
            // Generated controls declare their regions as display-space [x0, y0, x1, y1] ...
            for r in e["regions"].as_array().into_iter().flatten() {
                let kind = r["kind"].as_str().unwrap();
                if !kind.starts_with("sensitive") {
                    continue;
                }
                let c: Vec<f64> = r["rectTopLeft"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap())
                    .collect();
                let rect = Rect {
                    x: c[0],
                    y: c[1],
                    width: c[2] - c[0],
                    height: c[3] - c[1],
                };
                regions.push((kind.to_string(), r["page"].as_u64().unwrap() as u32, rect));
            }
            // ... HDDN's as fractions of the display size: the boxes it redacts itself.
            for t in e["targets"].as_array().into_iter().flatten() {
                let (w, h) = (
                    t["displaySize"][0].as_f64().unwrap(),
                    t["displaySize"][1].as_f64().unwrap(),
                );
                let b = |k: &str| t["box"][k].as_f64().unwrap();
                let rect = Rect {
                    x: b("x") * w,
                    y: b("y") * h,
                    width: b("width") * w,
                    height: b("height") * h,
                };
                regions.push((
                    "region".to_string(),
                    t["pageIndex"].as_u64().unwrap() as u32,
                    rect,
                ));
            }
            Case {
                id: f["id"].as_str().unwrap().into(),
                path: f["path"].as_str().unwrap().into(),
                password: f["password"].as_str().map(String::from),
                features: strings(&f["features"]),
                secrets,
                keep: [
                    strings(&e["mustKeepText"]),
                    strings(&e["mustKeepRenderedText"]),
                ]
                .concat(),
                regions,
            }
        })
        .collect()
}

/// The whole path of one export through the public engine: open, redactions over every seeded
/// secret (found by Foliark's own text) and region, native export, hand-over. The first typed
/// error stops it; no stage after an error runs, so there are no bytes to take.
fn export(c: &Case, input: &[u8], redact: bool) -> Result<(Vec<u8>, ExportReport), PdfError> {
    load_cjk_pack(); // the SDK registers a document's CMaps at open
    let mut e = Engine::new(Limits::default());
    let result = (|| {
        let (doc, info) = e.open(input.to_vec(), c.password.as_deref(), false)?;
        if redact {
            let mut reds = Vec::new();
            for p in 0..info.page_count {
                let t = e.extract_text(&doc, p, ExtractOptions::default());
                let text = run(&mut e, t)?.text;
                for s in &c.secrets {
                    for (at, _) in text.match_indices(s.as_str()) {
                        let start = text[..at].encode_utf16().count() as u32;
                        reds.push(Redaction::Text {
                            id: format!("t{p}-{}", reds.len()),
                            page_index: p,
                            start,
                            end: start + s.encode_utf16().count() as u32,
                            fill: None,
                        });
                    }
                }
            }
            for (i, (_, page, rect)) in c.regions.iter().enumerate() {
                reds.push(Redaction::Region {
                    id: format!("g{i}"),
                    page_index: *page,
                    rect: *rect,
                    fill: None,
                });
            }
            if !reds.is_empty() {
                let t = e.apply_changes(
                    &doc,
                    ChangeSet {
                        base_revision: 0,
                        text_edits: None,
                        redactions: Some(reds),
                    },
                );
                let r = run(&mut e, t)?;
                assert!(r.accepted, "{}: {:?}", c.id, r.rejected);
            }
        }
        let t = e.prepare_export(&doc, ExportMode::Native, ExportPolicy::default());
        let plan = run(&mut e, t)?;
        let bytes = e.take_export(&doc, &plan.plan_id)?;
        assert_eq!(plan.byte_length as usize, bytes.len());
        Ok((bytes, plan.report))
    })();
    if result.is_err() {
        assert_eq!(
            e.stats().plan_bytes,
            0,
            "{}: a refusal holds no bytes",
            c.id
        );
    }
    result
}

/// Sanitize-only reports every feature the fixture has that export removes or flattens.
fn disclosed(c: &Case, r: &ExportReport) -> Vec<String> {
    let removed = |i| r.removed.get(&i).copied().unwrap_or(0) > 0;
    let loss = |l| r.losses.iter().any(|x| x.code == l && x.count > 0);
    let mut missing = Vec::new();
    for f in &c.features {
        let ok = match f.as_str() {
            "metadata" | "xmp" => removed(SanitizedItem::Metadata),
            "embedded-file" => removed(SanitizedItem::EmbeddedFiles),
            "javascript" | "openaction" | "additional-actions" | "uri-action" => {
                removed(SanitizedItem::Actions)
            }
            "outline" => removed(SanitizedItem::Outlines),
            "thumbnail" => removed(SanitizedItem::Thumbnails),
            "actualtext" | "structure-alt" => removed(SanitizedItem::Structure),
            "ocg" => removed(SanitizedItem::OcgContent) || r.flattened.ocg_layers > 0,
            "widget" => r.flattened.widgets > 0,
            "annotation" => r.flattened.annotations > 0 || loss(LossCode::AnnotationsRemoved),
            "invisible-text" => {
                r.kept.invisible_text_clusters > 0 || removed(SanitizedItem::InvisibleText)
            }
            "encrypted" => loss(LossCode::EncryptionRemoved),
            _ => true,
        };
        if !ok {
            missing.push(format!("{} not reported", f));
        }
    }
    missing
}

#[test]
fn every_fixture_exports_safely_or_refuses_with_its_typed_code() {
    let dir = tmp("strict_native", "x").parent().unwrap().to_path_buf();
    let (mut rows, mut failures, mut jobs) = (Vec::new(), Vec::new(), Vec::new());
    let (mut supported, mut refused) = (0, Vec::new());
    let all = cases();
    for c in &all {
        let input = std::fs::read(repo().join("tests/fixtures").join(&c.path)).unwrap();
        let want = REFUSED
            .iter()
            .find(|(id, _)| *id == c.id)
            .map(|(_, code)| *code);
        let (red, san) = (export(c, &input, true), export(c, &input, false));
        match (want, red, san) {
            (Some(code), Err(a), Err(b)) if a.code == code && b.code == code => {
                refused.push(format!("{} {:?}", c.id, code));
                rows.push(format!(
                    "| {} | refused {:?} | no bytes (both runs) | {} | - | - |",
                    c.id,
                    code,
                    sha(&input)
                ));
            }
            (Some(code), a, b) => failures.push(format!(
                "{}: want {code:?} twice, got {:?} / {:?}",
                c.id,
                a.map(|x| x.1.pages),
                b.map(|x| x.1.pages)
            )),
            (None, Ok((red, _)), Ok((san, report))) => {
                supported += 1;
                for (k, b) in [("redacted", &red), ("sanitize", &san)] {
                    std::fs::write(dir.join(format!("{}.{k}.pdf", c.id)), b).unwrap();
                }
                for m in disclosed(c, &report) {
                    failures.push(format!("{}: {m}", c.id));
                }
                rows.push(format!(
                    "| {} | supported | CHECKS | {} | {} | {} |",
                    c.id,
                    sha(&input),
                    sha(&red),
                    sha(&san)
                ));
                jobs.push(json!({
                    "id": c.id, "input": repo().join("tests/fixtures").join(&c.path), "password": c.password,
                    "secrets": c.secrets, "keep": c.keep,
                    "regions": c.regions.iter().map(|(k, p, r)| json!({"kind": k, "page": p, "rect": [r.x, r.y, r.x + r.width, r.y + r.height]})).collect::<Vec<_>>(),
                    "flags": {"layers": c.features.iter().any(|f| f == "ocg"), "actualtext": c.features.iter().any(|f| f == "actualtext")},
                    "redacted": dir.join(format!("{}.redacted.pdf", c.id)), "sanitize": dir.join(format!("{}.sanitize.pdf", c.id)),
                }));
            }
            (None, a, b) => failures.push(format!(
                "{}: expected a supported export, got {:?} / {:?}",
                c.id,
                a.err().map(|e| (e.code, e.details)),
                b.err().map(|e| (e.code, e.details))
            )),
        }
    }
    // Negative controls: the same checks on the sanitize-only bytes presented as the redacted output (the secrets are
    // still there) must find them, by every route that applies.
    let controls = ["mixed-regions", "ocr-over-image", "jpeg-payload-secret"];
    for id in controls {
        let mut j = jobs.iter().find(|j| j["id"] == id).unwrap().clone();
        j["id"] = json!(format!("neg:{id}"));
        j["redacted"] = j["sanitize"].clone();
        jobs.push(j);
    }
    let jobs_file = dir.join("cases.json");
    std::fs::write(&jobs_file, serde_json::to_vec(&jobs).unwrap()).unwrap();
    let (ok, log) = python(&[
        "tests/integration/strict_native/check.py",
        jobs_file.to_str().unwrap(),
    ]);
    let results: Value = log
        .rsplit_once("\n@@")
        .map(|(_, j)| serde_json::from_str(j).unwrap())
        .unwrap_or_else(|| panic!("check.py: {log}"));
    assert!(ok, "check.py: {log}");
    let mut checked = 0;
    for (id, r) in results.as_object().unwrap() {
        if let Some(case) = id.strip_prefix("neg:") {
            let findings = r["redacted"]["objects"].to_string();
            let need: &[&str] = match case {
                "mixed-regions" => &["IMAGE_REGION", "VECTOR_REGION", "STREAM", "TEXT_FITZ"],
                "ocr-over-image" => &["STREAM", "TEXT_FITZ"],
                _ => &["IMAGE_REGION", "STREAM"],
            };
            for code in need {
                if !findings.contains(code) {
                    failures.push(format!(
                        "negative control {case}: {code} not reported: {findings}"
                    ));
                }
            }
            continue;
        }
        for (k, v) in r.as_object().unwrap() {
            let found = |x: &Value| x.as_array().is_some_and(|a| !a.is_empty());
            if let Some(o) = v.as_object() {
                for (sub, list) in o {
                    if found(list) {
                        failures.push(format!("{id} {k}/{sub}: {list}"));
                    }
                }
            } else if k != "sanitize_gone" && found(v) {
                failures.push(format!("{id} {k}: {v}"));
            }
        }
        checked += 1;
    }
    for row in &mut rows {
        *row = row.replace(
            "CHECKS",
            "input control, objects, image/vector regions, fidelity, words",
        );
    }
    println!("{}", rows.join("\n"));
    println!("negative controls caught: {controls:?}");
    println!(
        "cases {} supported {supported} refused {} {:?}",
        all.len(),
        refused.len(),
        refused
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(
        (checked, supported + refused.len()),
        (supported, all.len()),
        "a case was skipped"
    );
}
