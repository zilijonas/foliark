//! Redaction planning (spec §4, §5; NEW-32): what a committed redaction list takes out of the
//! working copy, classified and disclosed before export. Display space throughout.
mod common;
use common::{Font, pdf_bytes};
use foliark_engine::redaction::plan::{ClusterReason, RedactionPlan, VectorClass};
use foliark_engine::{Binding, BoxTask, Committed, Document, Engine, Step, StoredRedaction};
use foliark_model::{
    ChangeSet, ErrorCode, Limits, LossCode, PdfError, Rect, Redaction, SearchOptions, SharedKind,
};
use pdf_writer::types::AnnotationType;
use pdf_writer::{Finish, Name, Pdf, Ref};

const R: fn(f64, f64, f64, f64) -> Rect = |x, y, width, height| Rect {
    x,
    y,
    width,
    height,
};

fn run<T>(e: &mut Engine, mut task: BoxTask<T>) -> Result<T, PdfError> {
    loop {
        if let Step::Done(v) = task.step(e, 1000)? {
            return Ok(v);
        }
    }
}

fn apply(e: &mut Engine, doc: &str, base: u32, list: Vec<Redaction>) -> Result<(), PdfError> {
    let task = e.apply_changes(
        doc,
        ChangeSet {
            base_revision: base,
            text_edits: None,
            redactions: Some(list),
        },
    );
    let r = run(e, task)?;
    assert!(r.accepted, "{:?}", r.rejected);
    Ok(())
}

fn region(id: &str, page: u32, rect: Rect) -> Redaction {
    Redaction::Region {
        id: id.into(),
        page_index: page,
        rect,
        fill: None,
    }
}

/// The plan of the engine's current revision (no edits: the working copy is the source).
fn engine_plan(e: &Engine, doc: &str, bytes: &[u8]) -> RedactionPlan {
    let working = Document::open(bytes.to_vec(), None).unwrap();
    RedactionPlan::build(&working, e.committed(doc).unwrap(), e.binding(doc).unwrap()).unwrap()
}

fn stored(id: &str, page: u32, rect: Rect) -> StoredRedaction {
    StoredRedaction {
        id: id.into(),
        page_index: page,
        regions: vec![rect],
        fill: None,
        must_remove: vec![],
    }
}

fn plan_of(bytes: Vec<u8>, list: Vec<StoredRedaction>) -> RedactionPlan {
    let doc = Document::open(bytes, None).unwrap();
    let committed = Committed {
        edits: vec![],
        redactions: list,
    };
    let binding = Binding {
        doc_id: "d".into(),
        generation: 1,
        revision: 1,
    };
    RedactionPlan::build(&doc, &committed, binding).unwrap()
}

fn glyphs(plan: &RedactionPlan, page: u32) -> Vec<u32> {
    let p = plan.pages.iter().find(|p| p.page_index == page).unwrap();
    let mut g: Vec<u32> = p.clusters.iter().map(|c| c.id.glyph).collect();
    g.sort_unstable();
    g
}

const CAT: &str = "BT /F1 10 Tf 1 0 0 1 20 300 Tm (cat dog cat) Tj ET";

#[test]
fn a_region_or_one_hit_plans_one_occurrence_and_find_all_plans_every_hit() {
    let bytes = pdf_bytes(
        &[CAT, "BT /F1 10 Tf 1 0 0 1 20 300 Tm (cat) Tj ET"],
        &Font::default(),
        0,
    );
    let mut e = Engine::new(Limits::default());
    let doc = e.open(bytes.clone(), None, false).unwrap().0;
    let hits = {
        let task = e.search(&doc, "cat", SearchOptions::default());
        run(&mut e, task).unwrap()
    };
    let spans: Vec<_> = hits
        .iter()
        .map(|h| (h.page_index, h.range.start, h.range.end))
        .collect();
    assert_eq!(spans, [(0, 0, 3), (0, 8, 11), (1, 0, 3)]);
    let text = |id: &str, h: &foliark_model::SearchHit| Redaction::Text {
        id: id.into(),
        page_index: h.page_index,
        start: h.range.start,
        end: h.range.end,
        fill: None,
    };

    // Selected occurrence: the first hit only. The identical word elsewhere is not planned.
    apply(&mut e, &doc, 0, vec![text("one", &hits[0])]).unwrap();
    let one = engine_plan(&e, &doc, &bytes);
    assert_eq!(one.pages.len(), 1);
    assert_eq!(glyphs(&one, 0), [0, 1, 2]);
    assert_eq!(one.report.clusters_removed, 3);

    // Find all: one redaction per hit, three pages-worth of occurrences, nine clusters.
    let all: Vec<_> = hits
        .iter()
        .enumerate()
        .map(|(i, h)| text(&format!("hit{i}"), h))
        .collect();
    apply(&mut e, &doc, 1, all).unwrap();
    let all = engine_plan(&e, &doc, &bytes);
    assert_eq!(
        all.pages.iter().map(|p| p.page_index).collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(glyphs(&all, 0), [0, 1, 2, 8, 9, 10]);
    assert_eq!(glyphs(&all, 1), [0, 1, 2]);
    assert_eq!(all.report.clusters_removed, 9);
    assert_eq!(all.report.regions, 3);

    // A region names its page: the same word on page 1 stays out of the plan.
    let plan = plan_of(bytes, vec![stored("r", 0, R(19.9, 90.0, 15.1, 14.0))]);
    assert_eq!(plan.pages.len(), 1);
    assert_eq!(
        glyphs(&plan, 0),
        [0, 1, 2],
        "the space at x 35 only touches"
    );
}

#[test]
fn a_ligature_is_one_cluster_removed_whole_and_the_two_percent_edge_holds() {
    let font = Font {
        to_unicode: vec![(1, "fi")],
        unnamed: false,
    };
    // "a" 20..25, the fi glyph 25..30, "b" 30..35, baseline display y 112.
    let bytes = pdf_bytes(
        &["BT /F1 10 Tf 1 0 0 1 20 288 Tm (a\\001b end) Tj ET"],
        &font,
        0,
    );
    let one = |w: f64| plan_of(bytes.clone(), vec![stored("r", 0, R(25.0, 100.0, w, 20.0))]);

    let below = one(0.09);
    assert!(
        below.pages[0].clusters.is_empty(),
        "1.8% of the glyph is below the rule"
    );
    let plan = one(0.11);
    let p = &plan.pages[0];
    assert_eq!(p.clusters.len(), 1);
    let c = &p.clusters[0];
    assert!(c.multi_codepoint && c.coverage > 0.02 && c.coverage < 0.03);
    assert_eq!(c.reason, ClusterReason::Overlap);
    // The disclosure widens to the whole glyph box, not the sliver the caller drew.
    let r = &p.regions[0];
    assert_eq!(r.partial_clusters, 1);
    assert!((r.effective.x - 25.0).abs() < 1e-6 && (r.effective.width - 5.0).abs() < 1e-6);
    assert!((r.requested.width - 0.11).abs() < 1e-9);

    // A text range that cuts the ligature in half still removes (and names) the cluster.
    let mut e = Engine::new(Limits::default());
    let doc = e.open(bytes.clone(), None, false).unwrap().0;
    let cut = Redaction::Text {
        id: "t".into(),
        page_index: 0,
        start: 1,
        end: 2,
        fill: None,
    };
    apply(&mut e, &doc, 0, vec![cut]).unwrap();
    let plan = engine_plan(&e, &doc, &bytes);
    assert_eq!(glyphs(&plan, 0), [1]);
    assert!(plan.pages[0].clusters[0].multi_codepoint);
}

/// 200 x 200 page, the image and the Form are shared objects; `contents` are the page streams.
fn graphics(contents: &[&str], annots: bool) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let (tree, im, fm) = (Ref::new(2), Ref::new(3), Ref::new(4));
    pdf.catalog(Ref::new(1)).pages(tree);
    let kids: Vec<Ref> = (0..contents.len() as i32)
        .map(|i| Ref::new(10 + 2 * i))
        .collect();
    pdf.pages(tree)
        .kids(kids.iter().copied())
        .count(kids.len() as i32);
    let mut img = pdf.image_xobject(im, &[200u8; 12]);
    img.width(2).height(2).bits_per_component(8);
    img.color_space().device_rgb();
    img.finish();
    let mut f = pdf.form_xobject(fm, b"55 105 10 10 re f");
    f.bbox(pdf_writer::Rect::new(0.0, 0.0, 200.0, 200.0));
    f.finish();
    for (i, c) in contents.iter().enumerate() {
        let mut p = pdf.page(kids[i]);
        p.parent(tree)
            .media_box(pdf_writer::Rect::new(0.0, 0.0, 200.0, 200.0))
            .contents(Ref::new(11 + 2 * i as i32));
        if annots {
            p.annotations([Ref::new(5), Ref::new(6)]);
        }
        let mut res = p.resources();
        let mut x = res.x_objects();
        x.pair(Name(b"Im"), im);
        x.pair(Name(b"Fm"), fm);
        x.finish();
        res.finish();
        p.finish();
        pdf.stream(Ref::new(11 + 2 * i as i32), c.as_bytes());
    }
    for (r, rect) in [
        (5, pdf_writer::Rect::new(90.0, 140.0, 130.0, 160.0)),
        (6, pdf_writer::Rect::new(150.0, 10.0, 170.0, 30.0)),
    ] {
        pdf.annotation(Ref::new(r))
            .subtype(AnnotationType::Square)
            .rect(rect);
    }
    pdf.finish()
}

// Region (display) x 50..100, y 50..100 = user y 100..150.
const REGION: Rect = Rect {
    x: 50.0,
    y: 50.0,
    width: 50.0,
    height: 50.0,
};

#[test]
fn graphics_are_classified_by_how_they_meet_the_region() {
    let page = "\
1 M\n\
60 110 20 20 re f\n\
80 110 40 20 re f\n\
60 120 m 120 120 l 90 140 l f\n\
4 w 40 112 30 20 re S\n\
4 w 60 151 m 90 151 l S\n\
1 w 60 151 m 90 151 l S\n\
70 105 5 5 re 90 105 20 5 re f\n\
150 10 10 10 re f\n\
q 40 0 0 40 80 110 cm /Im Do Q\n\
q 20 0 0 20 150 10 cm /Im Do Q\n\
q /Fm Do Q\n";
    let plan = plan_of(graphics(&[page], true), vec![stored("r", 0, REGION)]);
    let p = &plan.pages[0];
    let classes: Vec<_> = p.vectors.iter().map(|v| (v.subpath, v.class)).collect();
    use VectorClass::{Collateral, Cut, Inside};
    // Rect inside; rect crossing (cut); triangle (a filled path is one unit: cut); stroked rect
    // (collateral); the 4 pt line whose stroke reaches in from y 49 (collateral; the 1 pt line
    // does not; the miter limit is 1, a larger one widens every stroke's reach by that factor,
    // spec 5.4); the two subpaths of one `re re f` are one unit (cut); then the Form's own rect.
    assert_eq!(
        classes,
        [
            (0, Inside),
            (0, Cut),
            (0, Cut),
            (0, Collateral),
            (0, Collateral),
            (0, Cut),
            (0, Inside)
        ]
    );
    // The plan only knows which paths may reach the region: cuts are counted from bounds.
    let r = &plan.report;
    assert_eq!((r.vector_subpaths_removed, r.vector_paths_cut), (4, 3));
    assert_eq!(r.collateral.len(), 2);
    assert!(r.collateral.iter().all(|c| c.page_index == 0));
    // A stroke reaches in by half its width: the collateral box of the 4 pt line is 4 pt tall.
    let line = r.collateral[1].bounds;
    assert!((line.height - 4.0).abs() < 1e-6 && (line.y - 47.0).abs() < 1e-6);

    // Only the image that meets the region is erased; it is drawn twice, so it needs a clone,
    // and the occurrence outside survives on the same page.
    assert_eq!(p.images.len(), 1);
    assert!(p.images[0].needs_clone);
    assert_eq!((r.images_erased, r.images_cloned), (1, 1));
    let survivors: Vec<_> = r
        .shared_survivors
        .iter()
        .filter(|s| s.kind == SharedKind::Image)
        .collect();
    assert_eq!(survivors.len(), 1);
    assert_eq!(
        (&survivors[0].redacted_on, &survivors[0].untouched_on),
        (&vec![0], &vec![0])
    );

    // The Form's rectangle is inside the region: the occurrence is planned, unshared.
    assert_eq!(p.forms.len(), 1);
    assert!(!p.forms[0].shared);

    // Annotations under the region only.
    assert_eq!(p.annotations.len(), 1);
    assert_eq!(p.annotations[0].subtype, "Square");

    // Losses known before export.
    let code = |c| plan.losses.iter().find(|l| l.code == c).map(|l| l.count);
    assert_eq!(code(LossCode::VectorCollateral), Some(2));
    assert_eq!(code(LossCode::ImageReencoded), Some(1));
    assert_eq!(p.regions[0].vectors, 7);
}

#[test]
fn a_resource_shared_with_an_untouched_page_lists_its_survivors() {
    let page = "q 40 0 0 40 60 110 cm /Im Do Q q /Fm Do Q";
    let bytes = graphics(&[page, page], false);
    let only_first = plan_of(bytes.clone(), vec![stored("r", 0, REGION)]);
    assert_eq!(only_first.pages.len(), 1);
    let r = &only_first.report;
    assert_eq!(
        (r.images_erased, r.images_cloned, r.forms_cloned),
        (1, 1, 1)
    );
    assert!(only_first.pages[0].forms[0].shared);
    let mut kinds: Vec<_> = r
        .shared_survivors
        .iter()
        .map(|s| (s.kind, s.redacted_on.clone(), s.untouched_on.clone()))
        .collect();
    kinds.sort_by_key(|k| k.0 == SharedKind::Form);
    assert_eq!(
        kinds,
        [
            (SharedKind::Image, vec![0], vec![1]),
            (SharedKind::Form, vec![0], vec![1])
        ]
    );

    // Both pages redacted: both occurrences change, nothing survives, both still need clones.
    let both = plan_of(bytes, vec![stored("a", 0, REGION), stored("b", 1, REGION)]);
    assert!(both.report.shared_survivors.is_empty());
    assert_eq!(
        (both.report.images_cloned, both.report.forms_cloned),
        (2, 2)
    );
}

#[test]
fn a_plan_is_refused_once_the_revision_moves() {
    let bytes = pdf_bytes(&[CAT], &Font::default(), 0);
    let mut e = Engine::new(Limits::default());
    let doc = e.open(bytes.clone(), None, false).unwrap().0;
    apply(
        &mut e,
        &doc,
        0,
        vec![region("a", 0, R(19.9, 90.0, 15.1, 14.0))],
    )
    .unwrap();
    let plan = engine_plan(&e, &doc, &bytes);
    plan.ensure_current(&e).unwrap();
    apply(&mut e, &doc, 1, vec![]).unwrap();
    assert_eq!(
        plan.ensure_current(&e).unwrap_err().code,
        ErrorCode::StalePlan
    );
    // A plan built for the new revision is current; the closed document's plan never is.
    let fresh = engine_plan(&e, &doc, &bytes);
    fresh.ensure_current(&e).unwrap();
    e.close(&doc);
    assert_eq!(
        fresh.ensure_current(&e).unwrap_err().code,
        ErrorCode::StalePlan
    );
}
