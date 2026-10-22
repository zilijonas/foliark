//! Atomic revisions and declarative ChangeSets (spec §4): stale base, all-or-nothing, list
//! replace/keep/undo, text redactions resolved to per-line regions, revision stamping.
mod common;
use common::{Font, close, pdf_bytes};
use foliark_engine::{Applied, BoxTask, EditApplier, Engine, Step};
use foliark_model::{
    ChangeResult, ChangeSet, ErrorCode, ExtractOptions, Limits, PdfError, Rect, Redaction,
    RejectedChange, TextEdit,
};

/// Page 0: "one two" / "a<fi>b end" (the second line holds a ligature cluster, "fi" at 9..11 of
/// "one two afib end"). Page 1: "page two".
fn fixture() -> Vec<u8> {
    let font = Font {
        to_unicode: vec![(1, "fi")],
        unnamed: false,
    };
    pdf_bytes(
        &[
            "BT /F1 10 Tf 1 0 0 1 20 300 Tm (one two) Tj 1 0 0 1 20 288 Tm (a\\001b end) Tj ET",
            "BT /F1 10 Tf 1 0 0 1 20 300 Tm (page two) Tj ET",
        ],
        &font,
        0,
    )
}

fn open(e: &mut Engine) -> String {
    e.open(fixture(), None, false).unwrap().0
}

fn run<T>(e: &mut Engine, mut task: BoxTask<T>) -> Result<T, PdfError> {
    loop {
        if let Step::Done(v) = task.step(e, 1000)? {
            return Ok(v);
        }
    }
}

fn apply(e: &mut Engine, doc: &str, cs: ChangeSet) -> Result<ChangeResult, PdfError> {
    let task = e.apply_changes(doc, cs);
    run(e, task)
}

fn text(e: &mut Engine, doc: &str, page: u32) -> (u32, String) {
    let task = e.extract_text(doc, page, ExtractOptions::default());
    let t = run(e, task).unwrap();
    (t.revision, t.text)
}

const R: fn(f64, f64, f64, f64) -> Rect = |x, y, width, height| Rect {
    x,
    y,
    width,
    height,
};

fn region(id: &str, page: u32, rect: Rect) -> Redaction {
    Redaction::Region {
        id: id.into(),
        page_index: page,
        rect,
        fill: None,
    }
}

fn text_red(id: &str, page: u32, start: u32, end: u32) -> Redaction {
    Redaction::Text {
        id: id.into(),
        page_index: page,
        start,
        end,
        fill: None,
    }
}

fn reds(base: u32, list: Vec<Redaction>) -> ChangeSet {
    ChangeSet {
        base_revision: base,
        text_edits: None,
        redactions: Some(list),
    }
}

fn edit(id: &str) -> TextEdit {
    TextEdit {
        id: id.into(),
        page_index: 0,
        rects: vec![R(20.0, 90.0, 40.0, 10.0)],
        text: "new".into(),
        font: None,
    }
}

fn ids(e: &Engine, doc: &str) -> Vec<String> {
    e.committed(doc)
        .unwrap()
        .op_ids()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_stale_base_is_refused_and_a_committed_changeset_cannot_commit_twice() {
    let mut e = Engine::new(Limits::default());
    let doc = open(&mut e);
    let err = apply(
        &mut e,
        &doc,
        reds(3, vec![region("a", 0, R(0.0, 0.0, 10.0, 10.0))]),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleRevision);
    assert_eq!(
        (e.revision(&doc).unwrap(), ids(&e, &doc)),
        (0, Vec::<String>::new())
    );

    let cs = reds(0, vec![region("a", 0, R(0.0, 0.0, 10.0, 10.0))]);
    let first = apply(&mut e, &doc, cs.clone()).unwrap();
    assert!(first.accepted);
    assert_eq!((first.revision, first.changed_pages), (1, vec![0]));
    // The same request again (a retry after a lost acknowledgment) must not commit a second time.
    let err = apply(&mut e, &doc, cs).unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleRevision);
    assert_eq!(
        (e.revision(&doc).unwrap(), ids(&e, &doc)),
        (1, vec!["a".to_owned()])
    );
}

#[test]
fn one_bad_item_rejects_the_whole_changeset_and_nothing_changes() {
    let mut e = Engine::new(Limits::default());
    let doc = open(&mut e);
    let good = region("keep", 0, R(0.0, 0.0, 10.0, 10.0));
    apply(&mut e, &doc, reds(0, vec![good.clone()])).unwrap();
    let before = e.committed(&doc).unwrap().clone();

    let nan = f64::NAN;
    let bad: Vec<(&str, Redaction)> = vec![
        ("page", region("x", 9, R(0.0, 0.0, 5.0, 5.0))),
        ("nan", region("x", 0, R(nan, 0.0, 5.0, 5.0))),
        ("empty rect", region("x", 0, R(0.0, 0.0, 0.0, 5.0))),
        (
            "fill",
            Redaction::Region {
                id: "x".into(),
                page_index: 0,
                rect: R(0.0, 0.0, 5.0, 5.0),
                fill: Some(Some([2.0, 0.0, 0.0])),
            },
        ),
        ("past the end", text_red("x", 0, 10, 99)),
        ("reversed", text_red("x", 0, 5, 2)),
        ("empty range", text_red("x", 0, 5, 5)),
        // Offset 7 is the separator between "two" and "afib": no cluster, so nothing to remove.
        ("only a separator", text_red("x", 0, 7, 8)),
        ("empty id", region("", 0, R(0.0, 0.0, 5.0, 5.0))),
    ];
    for (what, item) in bad {
        // Next to a valid item, so a partial commit would show.
        let cs = reds(1, vec![region("ok", 0, R(1.0, 1.0, 5.0, 5.0)), item]);
        let r = apply(&mut e, &doc, cs).unwrap();
        assert!(!r.accepted, "{what}");
        assert_eq!(r.revision, 1, "{what}");
        assert_eq!(r.rejected.len(), 1, "{what}");
        assert_eq!(r.rejected[0].code, ErrorCode::InvalidArgument, "{what}");
        assert!(r.changed_pages.is_empty() && r.layouts.is_empty(), "{what}");
        assert_eq!(*e.committed(&doc).unwrap(), before, "{what}");
        assert_eq!(e.revision(&doc).unwrap(), 1, "{what}");
    }
    // An id is one operation across both lists and within one list.
    let dup = reds(
        1,
        vec![
            region("d", 0, R(0.0, 0.0, 5.0, 5.0)),
            region("d", 1, R(0.0, 0.0, 5.0, 5.0)),
        ],
    );
    let r = apply(&mut e, &doc, dup).unwrap();
    assert_eq!((r.accepted, r.rejected.len()), (false, 1));
    let cross = ChangeSet {
        base_revision: 1,
        text_edits: Some(vec![edit("keep")]),
        redactions: None,
    };
    assert!(!apply(&mut e, &doc, cross).unwrap().accepted);
    assert_eq!(*e.committed(&doc).unwrap(), before);

    // With the default applier every edit is refused as not built, and that rejects the
    // redaction submitted with it too.
    let both = ChangeSet {
        base_revision: 1,
        text_edits: Some(vec![edit("e1"), edit("e2")]),
        redactions: Some(vec![region("r", 0, R(0.0, 0.0, 5.0, 5.0))]),
    };
    let r = apply(&mut e, &doc, both).unwrap();
    assert!(!r.accepted);
    assert_eq!(
        r.rejected
            .iter()
            .map(|x| (x.id.as_str(), x.code))
            .collect::<Vec<_>>(),
        [
            ("e1", ErrorCode::InvalidArgument),
            ("e2", ErrorCode::InvalidArgument)
        ]
    );
    let feature = r.rejected[0]
        .details
        .as_ref()
        .and_then(|d| d.feature.as_deref());
    assert_eq!(feature, Some("textEdits"));
    assert_eq!(
        (
            *e.committed(&doc).unwrap() == before,
            e.revision(&doc).unwrap()
        ),
        (true, 1)
    );
}

#[test]
fn lists_replace_when_present_keep_when_omitted_and_an_older_list_is_an_undo() {
    let mut e = Engine::new(Limits::default());
    let doc = open(&mut e);
    let (a, b) = (
        region("a", 0, R(0.0, 0.0, 10.0, 10.0)),
        region("b", 1, R(5.0, 5.0, 10.0, 10.0)),
    );
    let r1 = apply(&mut e, &doc, reds(0, vec![a.clone(), b.clone()])).unwrap();
    assert_eq!((r1.revision, r1.changed_pages), (1, vec![0, 1]));
    let v1 = e.committed(&doc).unwrap().clone();

    // Present textEdits (empty) with redactions omitted: the redaction list is kept.
    let keep = ChangeSet {
        base_revision: 1,
        text_edits: Some(vec![]),
        redactions: None,
    };
    let r = apply(&mut e, &doc, keep).unwrap();
    assert_eq!((r.accepted, r.revision, r.changed_pages), (true, 2, vec![]));
    assert_eq!(*e.committed(&doc).unwrap(), v1);

    // Present redactions replace the whole list: "b" disappears, only page 1 changed.
    let r = apply(&mut e, &doc, reds(2, vec![a.clone()])).unwrap();
    assert_eq!((r.revision, r.changed_pages), (3, vec![1]));
    assert_eq!(ids(&e, &doc), ["a"]);

    // Undo is resubmitting the older list. The revision still goes up; the content is back.
    let r = apply(&mut e, &doc, reds(3, vec![a, b])).unwrap();
    assert_eq!(
        (r.accepted, r.revision, r.changed_pages),
        (true, 4, vec![1])
    );
    assert_eq!(*e.committed(&doc).unwrap(), v1);
    // Clearing is an empty list, not an omitted one.
    apply(&mut e, &doc, reds(4, vec![])).unwrap();
    assert!(ids(&e, &doc).is_empty());
}

#[test]
fn a_text_redaction_resolves_to_one_region_per_line_and_the_clusters_it_touches() {
    let mut e = Engine::new(Limits::default());
    let doc = open(&mut e);
    // "one two afib end": 5..10 cuts "two" at the "w" and the "fi" ligature (9..11) in half.
    let r = apply(&mut e, &doc, reds(0, vec![text_red("t", 0, 5, 10)])).unwrap();
    assert!(r.accepted, "{r:?}");
    let [stored] = e.committed(&doc).unwrap().redactions.as_slice() else {
        panic!("one redaction")
    };
    // Line 1 "wo" (x 45..55) and line 2 "a" + the ligature (x 20..30), 10 pt tall boxes.
    let [l1, l2] = stored.regions.as_slice() else {
        panic!("{:?}", stored.regions)
    };
    assert!(
        close(l1.x, 45.0) && close(l1.width, 10.0) && close(l1.height, 10.0),
        "{l1:?}"
    );
    assert!(
        close(l2.x, 20.0) && close(l2.width, 10.0) && close(l2.height, 10.0),
        "{l2:?}"
    );
    assert!(
        close(l1.y - l2.y, -12.0),
        "line 1 sits 12 pt above line 2 in display space"
    );
    // Four clusters, the cut ligature whole ("b" and "e" are not touched). Ids are
    // (operator, index within it): "wo" are codes 5 and 6 of the first Tj, "a" and the
    // ligature codes 0 and 1 of the second.
    let glyphs: Vec<u32> = stored.must_remove.iter().map(|c| c.glyph).collect();
    assert_eq!(glyphs, [5, 6, 0, 1]);
    let ops = |i: usize| stored.must_remove[i].loc;
    assert_eq!((ops(0), ops(2)), (ops(1), ops(3)));
    assert_ne!(ops(0), ops(2));

    // The ids are the glyphs' identity in the file: a second engine resolving the same
    // selection from the same bytes gets the same ones.
    let mut other = Engine::new(Limits::default());
    let doc2 = open(&mut other);
    apply(&mut other, &doc2, reds(0, vec![text_red("t", 0, 5, 10)])).unwrap();
    assert_eq!(other.committed(&doc2).unwrap().redactions[0], *stored);
}

/// An applier that stands in for NEW-51: any edit list produces a one-line document with
/// "zzz" at x = 100, or (for an edit named "pages") a document with the wrong page count.
struct Swap;

impl EditApplier for Swap {
    fn apply(
        &self,
        _: &[u8],
        edits: &[TextEdit],
        _: &Limits,
    ) -> Result<Applied, Vec<RejectedChange>> {
        if let Some(e) = edits.iter().find(|e| e.id == "no") {
            return Err(vec![RejectedChange {
                id: e.id.clone(),
                code: ErrorCode::EditNoFit,
                details: None,
            }]);
        }
        let page = "BT /F1 10 Tf 1 0 0 1 100 200 Tm (zzz) Tj ET";
        let pages: &[&str] = if edits.iter().any(|e| e.id == "pages") {
            &[page]
        } else {
            &[page, page]
        };
        Ok(Applied {
            bytes: pdf_bytes(pages, &Font::default(), 0),
            layouts: vec![],
        })
    }
}

#[test]
fn edits_swap_the_working_copy_atomically_and_revisions_follow_the_commit() {
    let mut e = Engine::new(Limits::default()).with_edit_applier(Box::new(Swap));
    let doc = open(&mut e);
    assert_eq!(text(&mut e, &doc, 0), (0, "one two afib end".into()));
    let with = |base, edits, redactions| ChangeSet {
        base_revision: base,
        text_edits: edits,
        redactions,
    };

    // The applier rejects one edit: nothing changed, the revision and the text are the source's.
    let r = apply(
        &mut e,
        &doc,
        with(0, Some(vec![edit("ok"), edit("no")]), None),
    )
    .unwrap();
    assert_eq!(
        (r.accepted, r.revision, r.rejected[0].code),
        (false, 0, ErrorCode::EditNoFit)
    );
    assert_eq!(text(&mut e, &doc, 0), (0, "one two afib end".into()));
    // A working copy that lost a page is an engine bug, not a state to commit.
    let err = apply(&mut e, &doc, with(0, Some(vec![edit("pages")]), None)).unwrap_err();
    assert_eq!(
        (err.code, e.revision(&doc).unwrap()),
        (ErrorCode::Internal, 0)
    );

    // Accepted: extract reads the working copy and carries the new revision. The text
    // redaction in the same ChangeSet resolved against the BASE text (x 20..35 "one").
    let cs = with(0, Some(vec![edit("e")]), Some(vec![text_red("t", 0, 0, 3)]));
    let r = apply(&mut e, &doc, cs).unwrap();
    assert_eq!(
        (r.accepted, r.revision, r.changed_pages),
        (true, 1, vec![0])
    );
    assert_eq!(text(&mut e, &doc, 0), (1, "zzz".into()));
    let c = e.committed(&doc).unwrap();
    assert_eq!(
        (c.edits.len(), close(c.redactions[0].regions[0].x, 20.0)),
        (1, true)
    );
    assert_eq!(ids(&e, &doc), ["e", "t"]);
    // A render does not move the revision.
    let task = e.render(&doc, 0, Default::default());
    run(&mut e, task).unwrap();
    assert_eq!(e.revision(&doc).unwrap(), 1);

    // Clearing the edits (undo) brings the source back as the working copy at revision 2,
    // and the redaction list stays.
    let r = apply(&mut e, &doc, with(1, Some(vec![]), None)).unwrap();
    assert_eq!((r.accepted, r.revision), (true, 2));
    assert_eq!(text(&mut e, &doc, 0), (2, "one two afib end".into()));
    assert_eq!(ids(&e, &doc), ["t"]);
}

#[test]
fn bindings_name_the_revision_and_go_stale_on_commit_and_close() {
    let mut e = Engine::new(Limits::default()).with_generation(7);
    let doc = open(&mut e);
    let b0 = e.binding(&doc).unwrap();
    assert_eq!((b0.generation, b0.revision), (7, 0));
    assert!(e.is_current(&b0));
    apply(&mut e, &doc, reds(0, vec![])).unwrap();
    assert!(
        !e.is_current(&b0),
        "an empty accepted ChangeSet is still a new revision"
    );
    let b1 = e.binding(&doc).unwrap();
    assert!(e.is_current(&b1));
    e.close(&doc);
    assert!(!e.is_current(&b1));
}
