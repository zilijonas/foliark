//! Text results are bound to the revision they were computed at (spec §4): a long task cannot
//! straddle a commit, cached layouts survive only commits that cannot change text, and the layout
//! cache stays under its byte cap without changing any result.
mod common;
use common::{Font, pdf_bytes};
use foliark_engine::{Applied, BoxTask, Document, EditApplier, EditContext, Engine, Step};
use foliark_model::{
    ChangeSet, ErrorCode, ExtractOptions, Limits, PdfError, Rect, Redaction, RejectedChange,
    SearchOptions, TextEdit,
};

const PAGES: [&str; 3] = [
    "BT /F1 10 Tf 1 0 0 1 20 300 Tm (alpha one) Tj ET",
    "BT /F1 10 Tf 1 0 0 1 20 300 Tm (alpha two) Tj ET",
    "BT /F1 10 Tf 1 0 0 1 20 300 Tm (alpha three) Tj ET",
];

fn open(e: &mut Engine) -> String {
    e.open(pdf_bytes(&PAGES, &Font::default(), 0), None, false)
        .unwrap()
        .0
}

fn run<T>(e: &mut Engine, mut task: BoxTask<T>) -> Result<T, PdfError> {
    loop {
        if let Step::Done(v) = task.step(e, 1_000_000)? {
            return Ok(v);
        }
    }
}

fn commit(e: &mut Engine, doc: &str, cs: ChangeSet) {
    let task = e.apply_changes(doc, cs);
    assert!(run(e, task).unwrap().accepted);
}

fn redact(base: u32) -> ChangeSet {
    ChangeSet {
        base_revision: base,
        text_edits: None,
        redactions: Some(vec![Redaction::Region {
            id: format!("r{base}"),
            page_index: 0,
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
            fill: None,
        }]),
    }
}

fn text(e: &mut Engine, doc: &str, page: u32) -> (u32, String) {
    let task = e.extract_text(doc, page, ExtractOptions::default());
    let t = run(e, task).unwrap();
    (t.revision, t.text)
}

/// Stands in for the edit applier: every edit list yields three pages reading "zzz".
struct Swap;

impl EditApplier for Swap {
    fn apply(
        &self,
        _: &Document,
        _: &[TextEdit],
        _: &EditContext<'_>,
    ) -> Result<Applied, Vec<RejectedChange>> {
        let page = "BT /F1 10 Tf 1 0 0 1 100 200 Tm (zzz) Tj ET";
        Ok(Applied {
            bytes: pdf_bytes(&[page, page, page], &Font::default(), 0),
            layouts: vec![],
        })
    }
}

#[test]
fn a_task_cannot_finish_at_a_revision_it_did_not_start_at() {
    let mut e = Engine::new(Limits::default());
    let doc = open(&mut e);
    let mut search = e.search(&doc, "alpha", SearchOptions::default());
    // One page per step: the search is mid-flight when the commit lands.
    assert!(matches!(search.step(&mut e, 1).unwrap(), Step::Pending(_)));
    let extract = e.extract_text(&doc, 0, ExtractOptions::default());
    let render = e.render(&doc, 0, Default::default());
    commit(&mut e, &doc, redact(0));

    for code in [
        search.step(&mut e, 1).err().map(|e| e.code),
        run(&mut e, extract).err().map(|e| e.code),
        run(&mut e, render).err().map(|e| e.code),
    ] {
        assert_eq!(code, Some(ErrorCode::StaleRevision));
    }
    // Started after the commit, the same operations are fine and carry the new revision.
    assert_eq!(text(&mut e, &doc, 0), (1, "alpha one".into()));
    let task = e.search(&doc, "alpha", SearchOptions::default());
    let hits = run(&mut e, task).unwrap();
    assert!(hits.len() == 3 && hits.iter().all(|h| h.revision == 1));
    // A gone document is still DOCUMENT_GONE, not stale.
    let late = e.extract_text(&doc, 0, ExtractOptions::default());
    e.close(&doc);
    assert_eq!(
        run(&mut e, late).err().map(|e| e.code),
        Some(ErrorCode::DocumentGone)
    );
}

#[test]
fn a_redaction_commit_keeps_layouts_and_restamps_them_an_edit_commit_drops_them() {
    let mut e = Engine::new(Limits::default()).with_edit_applier(Box::new(Swap));
    let doc = open(&mut e);
    text(&mut e, &doc, 0);
    text(&mut e, &doc, 1);
    let before = e.stats();
    assert_eq!((before.documents, before.cache_entries), (1, 2));
    assert!(before.cache_bytes > 0);

    // Redactions never change text: nothing is re-extracted, the revision still moves.
    commit(&mut e, &doc, redact(0));
    assert_eq!(e.stats().cache_bytes, before.cache_bytes);
    assert_eq!(text(&mut e, &doc, 1), (1, "alpha two".into()));

    // A new working copy renumbers objects, so every cached glyph id is dropped and the next
    // read extracts the new file lazily.
    let edit = TextEdit {
        id: "e".into(),
        page_index: 0,
        rects: vec![Rect {
            x: 0.0,
            y: 0.0,
            width: 50.0,
            height: 50.0,
        }],
        text: "x".into(),
        font: None,
    };
    let cs = ChangeSet {
        base_revision: 1,
        text_edits: Some(vec![edit]),
        redactions: None,
    };
    commit(&mut e, &doc, cs);
    assert_eq!(e.stats().cache_entries, 0);
    assert_eq!(text(&mut e, &doc, 1), (2, "zzz".into()));
    assert_eq!(e.stats().cache_entries, 1);

    e.close(&doc);
    let after = e.stats();
    assert_eq!(
        (after.documents, after.cache_entries, after.cache_bytes),
        (0, 0, 0)
    );
}

#[test]
fn the_layout_cache_stays_under_its_cap_and_eviction_changes_no_result() {
    let mut roomy = Engine::new(Limits::default());
    let doc = open(&mut roomy);
    let expect: Vec<_> = (0..3).map(|p| text(&mut roomy, &doc, p)).collect();
    assert_eq!(roomy.stats().cache_evictions, 0);
    let one_page = roomy.stats().cache_bytes / 3;

    // Room for about one and a half pages: visiting all three must evict, and bytes stay under
    // the cap plus the entry that was just built.
    let cap = one_page + one_page / 2;
    let mut tight = Engine::new(Limits::default()).with_cache_bytes(cap);
    let doc = open(&mut tight);
    for round in 0..3 {
        for p in [0, 1, 2, 1, 0, 2] {
            assert_eq!(
                text(&mut tight, &doc, p),
                expect[p as usize],
                "round {round}"
            );
            let s = tight.stats();
            assert!(
                s.cache_bytes <= cap + one_page,
                "{} > {cap} + {one_page}",
                s.cache_bytes
            );
            assert!(s.cache_entries <= 2);
        }
    }
    assert!(tight.stats().cache_evictions > 0);
}
