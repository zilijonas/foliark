//! Shared harness of the vector redaction tests (redact_fills, redact_strokes): plan, redact,
//! write, then judge the OUTPUT with the Python oracles (tests/oracles) and a PyMuPDF render.
//! Regions are given in user space of page 0 so the same case runs under any /Rotate.
#![allow(dead_code)]
use foliark_engine::interpret::ItemKind;
use foliark_engine::redaction::plan::RedactionPlan;
use foliark_engine::redaction::vector::{VectorReport, redact_vectors};
use foliark_engine::writer::Writer;
use foliark_engine::{Binding, Committed, Document, StoredRedaction};
use foliark_model::{Limits, Matrix, PdfError, Point, Rect};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("redact-vector");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// The oracle venv's python (cwd = repo root); `(success, stdout+stderr)`.
pub fn python(args: &[&str]) -> (bool, String) {
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

/// A user-space box of page 0 as the display-space region it covers.
pub fn region(doc: &Document, x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    let m: Matrix = doc.page_model(0).unwrap().geometry.user_to_display;
    let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| m.apply(Point { x, y }));
    let (lo_x, hi_x) = pts
        .iter()
        .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.x), a.1.max(p.x)));
    let (lo_y, hi_y) = pts
        .iter()
        .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.y), a.1.max(p.y)));
    Rect {
        x: lo_x,
        y: lo_y,
        width: hi_x - lo_x,
        height: hi_y - lo_y,
    }
}

pub fn plan_for(doc: &Document, regions: &[Rect]) -> RedactionPlan {
    try_plan(doc, regions).unwrap()
}

fn try_plan(doc: &Document, regions: &[Rect]) -> Result<RedactionPlan, PdfError> {
    let committed = Committed {
        edits: vec![],
        redactions: regions
            .iter()
            .enumerate()
            .map(|(i, r)| StoredRedaction {
                id: format!("r{i}"),
                page_index: 0,
                regions: vec![*r],
                fill: None,
                must_remove: vec![],
            })
            .collect(),
    };
    let binding = Binding {
        doc_id: "d".into(),
        generation: 1,
        revision: 1,
    };
    RedactionPlan::build(doc, &committed, binding)
}

pub struct Run {
    pub plan: RedactionPlan,
    pub report: VectorReport,
    pub input: Vec<u8>,
    pub output: Vec<u8>,
    pub regions: Vec<Rect>,
    pub name: String,
}

/// Plan, redact, write. The report must agree with what the plan disclosed.
pub fn run(name: &str, input: Vec<u8>, regions: &[Rect]) -> Run {
    let doc = Document::open(input.clone(), None).unwrap();
    let plan = plan_for(&doc, regions);
    let mut w = Writer::new(doc.reader(), Limits::default());
    let report = redact_vectors(&doc, &plan, &mut w).unwrap();
    assert_eq!(
        report.subpaths_removed, plan.report.vector_subpaths_removed,
        "{name}: removed"
    );
    assert_eq!(
        report.rects_cut, plan.report.vector_rects_cut,
        "{name}: cut"
    );
    assert_eq!(
        report.collateral, plan.report.collateral,
        "{name}: collateral"
    );
    let output = w.write().unwrap();
    Document::open(output.clone(), None).unwrap();
    Run {
        plan,
        report,
        input,
        output,
        regions: regions.to_vec(),
        name: name.into(),
    }
}

/// The error planning or `redact_vectors` stops with.
pub fn refusal(input: Vec<u8>, regions: &[Rect], limits: Limits) -> PdfError {
    let doc = Document::open_with_limits(input, None, false, limits).unwrap();
    let plan = match try_plan(&doc, regions) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mut w = Writer::new(doc.reader(), Limits::default());
    redact_vectors(&doc, &plan, &mut w).unwrap_err()
}

impl Run {
    fn files(&self) -> (PathBuf, PathBuf) {
        let (i, o) = (
            tmp(&format!("{}.in.pdf", self.name)),
            tmp(&format!("{}.out.pdf", self.name)),
        );
        std::fs::write(&i, &self.input).unwrap();
        std::fs::write(&o, &self.output).unwrap();
        (i, o)
    }

    /// objects.py (PyMuPDF `get_drawings`): no painted geometry reaches into any region, and
    /// the same check on the INPUT does find some (the oracle is not vacuous).
    pub fn assert_no_geometry_in_regions(&self) {
        let (i, o) = self.files();
        let rects: Vec<String> = self
            .regions
            .iter()
            .map(|r| {
                format!(
                    "{{\"page\":0,\"rect\":[{},{},{},{}]}}",
                    r.x,
                    r.y,
                    r.x + r.width,
                    r.y + r.height
                )
            })
            .collect();
        let spec = tmp(&format!("{}.spec.json", self.name));
        std::fs::write(
            &spec,
            format!("{{\"vector_regions\":[{}]}}", rects.join(",")),
        )
        .unwrap();
        let check = |p: &Path| {
            python(&[
                "tests/oracles/objects.py",
                p.to_str().unwrap(),
                "--spec",
                spec.to_str().unwrap(),
            ])
        };
        let (ok, log) = check(&o);
        assert!(ok, "{}: objects.py on output: {log}", self.name);
        let (ok, log) = check(&i);
        assert!(
            !ok && log.contains("VECTOR_REGION"),
            "{}: oracle blind on input: {log}",
            self.name
        );
    }

    /// fidelity.py: everything outside the regions and the declared collateral renders alike.
    pub fn assert_rest_unchanged(&self) {
        let (i, o) = self.files();
        let mut rects: Vec<String> = self
            .regions
            .iter()
            .map(|r| (r.x, r.y, r.width, r.height))
            .chain(
                self.report
                    .collateral
                    .iter()
                    .map(|c| (c.bounds.x, c.bounds.y, c.bounds.width, c.bounds.height)),
            )
            .map(|(x, y, w, h)| {
                format!("0:{},{},{},{}", x - 0.5, y - 0.5, x + w + 0.5, y + h + 0.5)
            })
            .collect();
        let mut args = vec![
            "tests/oracles/fidelity.py",
            i.to_str().unwrap(),
            o.to_str().unwrap(),
        ];
        for r in &mut rects {
            args.extend(["--changed", r.as_str()]);
        }
        let (ok, log) = python(&args);
        assert!(ok, "{}: fidelity.py: {log}", self.name);
    }

    /// Inked pixels (any channel under 250 at 4x) inside region `n`, inset 0.6 pt, in
    /// (input, output), rendered by PyMuPDF. Needs an unrotated page.
    pub fn ink(&self, n: usize) -> (u32, u32) {
        let (i, o) = self.files();
        let r = self.regions[n];
        let code = "import sys, fitz\np = fitz.open(sys.argv[1])[0]\nx0, y0, x1, y1 = map(float, sys.argv[2:6])\n\
                    pix = p.get_pixmap(matrix=fitz.Matrix(4, 4), clip=fitz.Rect(x0, y0, x1, y1), alpha=False)\n\
                    s, c = pix.samples, pix.n\nprint(sum(1 for k in range(0, len(s), c) if min(s[k:k + c]) < 250))";
        let count = |p: &Path| {
            let b = [
                r.x + 0.6,
                r.y + 0.6,
                r.x + r.width - 0.6,
                r.y + r.height - 0.6,
            ]
            .map(|v| v.to_string());
            let (ok, log) = python(&["-c", code, p.to_str().unwrap(), &b[0], &b[1], &b[2], &b[3]]);
            assert!(ok, "{}: render: {log}", self.name);
            log.lines().last().unwrap().trim().parse::<u32>().unwrap()
        };
        (count(&i), count(&o))
    }

    /// Region `n` holds ink in the input and none in the output.
    pub fn assert_region_blank(&self, n: usize) {
        let (before, after) = self.ink(n);
        assert!(
            before > 0,
            "{}: region {n} is blank in the input, the check is vacuous",
            self.name
        );
        assert_eq!(
            after, 0,
            "{}: region {n} still shows {after} inked samples",
            self.name
        );
    }

    /// (clip-only items, painted path items) of the output's page 0.
    pub fn output_paths(&self) -> (usize, usize) {
        let m = Document::open(self.output.clone(), None)
            .unwrap()
            .page_model(0)
            .unwrap();
        let (mut clips, mut painted) = (0, 0);
        for i in &m.items {
            if let ItemKind::Path(p) = &i.kind {
                if p.clip_only {
                    clips += 1;
                } else {
                    painted += 1;
                }
            }
        }
        (clips, painted)
    }
}

/// A 300 x 400 page with `content` and no resources (inputs for `region` and `run`).
pub fn page(content: &str, rotate: i32) -> Vec<u8> {
    use pdf_writer::{Finish, Pdf, Rect, Ref};
    let mut pdf = Pdf::new();
    let (cat, tree, p, c) = (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4));
    pdf.catalog(cat).pages(tree);
    pdf.pages(tree).kids([p]).count(1);
    let mut page = pdf.page(p);
    page.parent(tree)
        .media_box(Rect::new(0.0, 0.0, 300.0, 400.0))
        .contents(c);
    if rotate != 0 {
        page.rotate(rotate);
    }
    page.resources();
    page.finish();
    pdf.stream(c, content.as_bytes());
    pdf.finish()
}

/// A closed circle of four Beziers (a glyph-like outline contour).
pub fn circle(cx: f64, cy: f64, r: f64) -> String {
    let k = 0.5523 * r;
    format!(
        "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h ",
        cx + r,
        cy,
        cx + r,
        cy + k,
        cx + k,
        cy + r,
        cx,
        cy + r,
        cx - k,
        cy + r,
        cx - r,
        cy + k,
        cx - r,
        cy,
        cx - r,
        cy - k,
        cx - k,
        cy - r,
        cx,
        cy - r,
        cx + k,
        cy - r,
        cx + r,
        cy - k,
        cx + r,
        cy
    )
}
