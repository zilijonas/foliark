//! Hand-assembled PDFs with images, and the runner shared by the image and mask redaction
//! tests: plan, erase, write, then read the output back with pikepdf (tests/oracles).
#![allow(dead_code)]
use foliark_engine::redaction::image::{ImageReport, redact_images};
use foliark_engine::redaction::plan::RedactionPlan;
use foliark_engine::writer::Writer;
use foliark_engine::{Binding, Committed, Document, StoredRedaction};
use foliark_model::{Limits, PdfError, Rect};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("redact-images");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// The oracle venv's python (cwd = repo root); `(success, stdout, stderr)`.
pub fn python(args: &[&str]) -> (bool, String, String) {
    let py = root().join("tests/oracles/.venv/bin/python");
    assert!(py.exists(), "oracle venv missing (tests/oracles/README.md)");
    let out = Command::new(py)
        .args(args)
        .current_dir(root())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into(),
        String::from_utf8_lossy(&out.stderr).into(),
    )
}

pub fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut o = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    o.extend_from_slice(data);
    o.extend_from_slice(b"\nendstream");
    o
}

pub struct Page {
    /// Extra page dictionary entries (`/Rotate 90 /CropBox [...]`).
    pub extra: String,
    pub content: Vec<u8>,
    pub xobjects: Vec<(String, usize)>,
}

pub fn page(extra: &str, content: impl AsRef<[u8]>, xobjects: &[(&str, usize)]) -> Page {
    Page {
        extra: extra.into(),
        content: content.as_ref().to_vec(),
        xobjects: xobjects.iter().map(|(n, o)| (n.to_string(), *o)).collect(),
    }
}

/// Object number of the first caller object for `pages` pages (1 catalog, 2 tree, then a page
/// and its content per page); `objs[k]` is object `first_obj(n) + k`.
pub fn first_obj(pages: usize) -> usize {
    3 + 2 * pages
}

/// 300 x 400 pages. `objs` are complete object bodies.
pub fn pdf(pages: &[Page], objs: &[Vec<u8>]) -> Vec<u8> {
    let mut bodies: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!(
            "<< /Type /Pages /Count {} /Kids [{}] >>",
            pages.len(),
            (0..pages.len())
                .map(|i| format!("{} 0 R", 3 + 2 * i))
                .collect::<Vec<_>>()
                .join(" ")
        )
        .into_bytes(),
    ];
    for (i, p) in pages.iter().enumerate() {
        let xo: String = p
            .xobjects
            .iter()
            .map(|(n, o)| format!("/{n} {o} 0 R"))
            .collect();
        bodies.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] {} /Contents {} 0 R /Resources << /XObject << {xo} >> >> >>",
                p.extra,
                4 + 2 * i
            )
            .into_bytes(),
        );
        bodies.push(stream("", &p.content));
    }
    bodies.extend(objs.iter().cloned());
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, b) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(b);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes(),
    );
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            bodies.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A distinct, never-black color for sample (i, j).
pub fn px(i: usize, j: usize) -> [u8; 3] {
    [
        40 + (i * 29 % 150) as u8,
        40 + (j * 31 % 150) as u8,
        50 + ((i * 7 + j * 11) % 120) as u8,
    ]
}

/// A `w` x `h` DeviceRGB image object (raw samples, no filter).
pub fn rgb_image(w: usize, h: usize, extra: &str) -> Vec<u8> {
    let data: Vec<u8> = (0..h)
        .flat_map(|j| (0..w).flat_map(move |i| px(i, j)))
        .collect();
    stream(
        &format!(
            "/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 {extra}"
        ),
        &data,
    )
}

pub fn committed(page: u32, regions: &[Rect]) -> Committed {
    Committed {
        edits: vec![],
        redactions: regions
            .iter()
            .enumerate()
            .map(|(i, r)| StoredRedaction {
                id: format!("r{i}"),
                page_index: page,
                regions: vec![*r],
                fill: None,
                must_remove: vec![],
            })
            .collect(),
    }
}

pub fn make_plan(doc: &Document, per_page: &[(u32, &[Rect])]) -> RedactionPlan {
    let mut all = Committed {
        edits: vec![],
        redactions: vec![],
    };
    for (p, rs) in per_page {
        let mut c = committed(*p, rs);
        for r in &mut c.redactions {
            r.id = format!("p{p}{}", r.id);
        }
        all.redactions.extend(c.redactions);
    }
    let binding = Binding {
        doc_id: "d".into(),
        generation: 1,
        revision: 1,
    };
    RedactionPlan::build(doc, &all, binding).unwrap()
}

pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

pub struct Out {
    pub bytes: Vec<u8>,
    pub report: ImageReport,
    pub plan: RedactionPlan,
    pub path: PathBuf,
}

/// Erase `per_page` regions from `input` and write the result to `target/.../<name>.pdf`.
pub fn run(name: &str, input: &[u8], per_page: &[(u32, &[Rect])]) -> Result<Out, PdfError> {
    let doc = Document::open(input.to_vec(), None).unwrap();
    let plan = make_plan(&doc, per_page);
    let mut w = Writer::new(doc.reader(), Limits::default());
    let report = redact_images(&doc, &plan, &mut w)?;
    let bytes = w.write()?;
    let path = tmp(&format!("{name}.pdf"));
    std::fs::write(&path, &bytes).unwrap();
    Ok(Out {
        bytes,
        report,
        plan,
        path,
    })
}

/// pikepdf's view of an output: per page `{xobject name: image}` plus every image object of the
/// file (reachable or not) as `all`. Sample data is hex of the decoded stream.
pub fn samples(path: &Path) -> Value {
    let (ok, out, err) = python(&["tests/oracles/samples.py", path.to_str().unwrap()]);
    assert!(ok, "samples.py failed: {err}");
    serde_json::from_str(&out).unwrap()
}

pub fn bytes_of(v: &Value) -> Vec<u8> {
    let s = v.as_str().unwrap();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// The decoded samples of a JPEG by PIL.
pub fn jpeg_samples(path: &Path) -> Vec<u8> {
    pil_samples("--jpeg", path)
}

/// The decoded RGBA samples of a JPEG 2000 file by PIL.
pub fn rgba_samples(path: &Path) -> Vec<u8> {
    pil_samples("--rgba", path)
}

fn pil_samples(mode: &str, path: &Path) -> Vec<u8> {
    let (ok, out, err) = python(&["tests/oracles/samples.py", mode, path.to_str().unwrap()]);
    assert!(ok, "{err}");
    bytes_of(&Value::String(out.trim().into()))
}

/// objects.py with an image-region spec (`regions`: (page, [x0,y0,x1,y1])). Returns findings.
pub fn objects_check(path: &Path, regions: &[(u32, [f64; 4])]) -> (bool, String) {
    let spec = serde_json::json!({
        "image_regions": regions.iter().map(|(p, r)| serde_json::json!({"page": p, "rect": r})).collect::<Vec<_>>()
    });
    let spec_path = path.with_extension("spec.json");
    std::fs::write(&spec_path, spec.to_string()).unwrap();
    let (ok, out, err) = python(&[
        "tests/oracles/objects.py",
        path.to_str().unwrap(),
        "--spec",
        spec_path.to_str().unwrap(),
    ]);
    (ok, format!("{out}{err}"))
}
