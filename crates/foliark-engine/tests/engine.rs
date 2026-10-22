//! Engine sessions (spec §13): doc table, revision stamping, stepping, atomic font registration.
use foliark_engine::{BoxTask, Engine, RenderOptions, Step};
use foliark_model::{
    ErrorCode, ExtractOptions, FontSource, FontStyle, Limits, PdfError, SearchOptions,
};
use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};

/// One Helvetica page per entry of `words`, each showing that word.
fn pdf(words: &[&str]) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let (cat, tree, font) = (Ref::new(1), Ref::new(2), Ref::new(3));
    pdf.catalog(cat).pages(tree);
    let kids: Vec<Ref> = (0..words.len() as i32)
        .map(|i| Ref::new(10 + 2 * i))
        .collect();
    pdf.pages(tree)
        .kids(kids.iter().copied())
        .count(words.len() as i32);
    pdf.type1_font(font).base_font(Name(b"Helvetica"));
    for (i, w) in words.iter().enumerate() {
        let content = Ref::new(11 + 2 * i as i32);
        let mut p = pdf.page(kids[i]);
        p.parent(tree)
            .media_box(Rect::new(0.0, 0.0, 200.0, 100.0))
            .contents(content);
        p.resources().fonts().pair(Name(b"F1"), font);
        p.finish();
        let mut c = Content::new();
        c.begin_text()
            .set_font(Name(b"F1"), 18.0)
            .next_line(10.0, 60.0)
            .show(Str(w.as_bytes()))
            .end_text();
        pdf.stream(content, &c.finish());
    }
    pdf.finish()
}

fn run<T>(e: &mut Engine, mut task: BoxTask<T>, budget: u64) -> (Result<T, PdfError>, u32) {
    let mut steps = 0;
    loop {
        steps += 1;
        match task.step(e, budget) {
            Ok(Step::Pending(_)) => {}
            Ok(Step::Done(v)) => return (Ok(v), steps),
            Err(err) => return (Err(err), steps),
        }
    }
}

fn code<T>(r: Result<T, PdfError>) -> ErrorCode {
    r.err().expect("an error").code
}

fn font(path: &str, family: &str) -> FontSource {
    let bytes = std::fs::read(format!(
        "{}/../foliark-text/tests/data/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    FontSource {
        bytes,
        family: family.into(),
        weight: 400,
        style: FontStyle::Normal,
    }
}

#[test]
fn documents_are_independent_handles_that_stop_at_close() {
    let mut e = Engine::new(Limits::default());
    let (a, info) = e.open(pdf(&["alpha", "beta"]), None, false).unwrap();
    let (b, _) = e.open(pdf(&["gamma"]), None, false).unwrap();
    assert_ne!(a, b);
    assert_eq!(info.page_count, 2);
    assert_eq!(e.pages(&a).unwrap().len(), 2);
    let task = e.extract_text(&b, 0, ExtractOptions::default());
    let text = run(&mut e, task, 1000).0.unwrap();
    assert_eq!(
        (text.doc_id.as_str(), text.revision, text.text.as_str()),
        (b.as_str(), 0, "gamma")
    );
    assert!(text.clusters.is_none());
    e.close(&a);
    e.close(&a); // idempotent
    assert_eq!(code(e.pages(&a)), ErrorCode::DocumentGone);
    assert_eq!(
        code(e.hit_test(&a, 0, foliark_model::Point { x: 0.0, y: 0.0 }, None)),
        ErrorCode::DocumentGone
    );
    assert_eq!(
        e.pages(&b).unwrap().len(),
        1,
        "closing one document leaves the other"
    );
}

#[test]
fn open_documents_are_capped() {
    let mut e = Engine::new(Limits {
        max_open_documents: 1,
        ..Limits::default()
    });
    let (a, _) = e.open(pdf(&["x"]), None, false).unwrap();
    let err = e.open(pdf(&["y"]), None, false).err().unwrap();
    assert_eq!(
        (
            err.code,
            err.details.unwrap().limit.as_deref().map(str::to_owned)
        ),
        (ErrorCode::LimitExceeded, Some("maxOpenDocuments".into()))
    );
    e.close(&a);
    assert!(
        e.open(pdf(&["y"]), None, false).is_ok(),
        "a slot frees on close"
    );
}

#[test]
fn search_steps_page_by_page_and_matches_the_one_shot_result() {
    let mut e = Engine::new(Limits::default());
    let words = ["needle", "hay", "needle", "hay", "needle"];
    let (d, _) = e.open(pdf(&words), None, false).unwrap();
    let task = e.search(&d, "needle", SearchOptions::default());
    let (hits, steps) = run(&mut e, task, 1);
    let hits = hits.unwrap();
    assert_eq!(
        hits.iter().map(|h| h.page_index).collect::<Vec<_>>(),
        [0, 2, 4]
    );
    assert!(
        hits.iter()
            .all(|h| h.revision == 0 && h.range.text == "needle")
    );
    assert_eq!(steps, 5, "budget 1 means one page per step");
    let task = e.search(&d, "needle", SearchOptions::default());
    let (again, steps) = run(&mut e, task, 100_000);
    assert_eq!((again.unwrap(), steps), (hits.clone(), 1));
    let capped = SearchOptions {
        max_hits: 2,
        ..SearchOptions::default()
    };
    let task = e.search(&d, "needle", capped);
    assert_eq!(run(&mut e, task, 1).0.unwrap().len(), 2);
    let bad = SearchOptions {
        pages: Some(vec![9]),
        ..SearchOptions::default()
    };
    let task = e.search(&d, "needle", bad);
    assert_eq!(code(run(&mut e, task, 1).0), ErrorCode::InvalidArgument);
}

#[test]
fn render_and_hit_test_use_the_session_document() {
    let mut e = Engine::new(Limits::default());
    let (d, _) = e.open(pdf(&["Hello"]), None, false).unwrap();
    let task = e.render(&d, 0, RenderOptions::at(2.0));
    let page = run(&mut e, task, 1).0.unwrap();
    assert_eq!(
        (page.header.width, page.header.height, page.data.len()),
        (400, 200, 400 * 200 * 4)
    );
    // The text sits at x 10.., baseline y 60 (PDF) = 40 (display): a point inside the "H".
    let hit = e
        .hit_test(&d, 0, foliark_model::Point { x: 14.0, y: 34.0 }, None)
        .unwrap()
        .expect("a hit");
    assert_eq!((hit.start, hit.end), (0, 1));
    assert!(
        e.hit_test(&d, 0, foliark_model::Point { x: 190.0, y: 95.0 }, None)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        code(e.hit_test(
            &d,
            0,
            foliark_model::Point {
                x: f64::NAN,
                y: 0.0
            },
            None
        )),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        code(e.hit_test(&d, 3, foliark_model::Point { x: 0.0, y: 0.0 }, None)),
        ErrorCode::InvalidArgument
    );
}

#[test]
fn font_registration_is_all_or_nothing_and_counted() {
    let mut e = Engine::new(Limits::default());
    let good = font("LiberationSans-Regular.ttf", "Liberation Sans");
    let junk = FontSource {
        bytes: b"not a font".to_vec(),
        family: "Junk".into(),
        weight: 400,
        style: FontStyle::Normal,
    };
    assert_eq!(
        code(e.register_fonts(vec![good.clone(), junk])),
        ErrorCode::FontUnsupported
    );
    assert_eq!(e.font_registry_version(), 0, "the good face was not kept");
    let reg = e.register_fonts(vec![good.clone()]).unwrap();
    assert_eq!(
        (reg.len(), reg[0].family.as_str(), e.font_registry_version()),
        (1, "Liberation Sans", 1)
    );
    let mut small = Engine::new(Limits {
        max_registered_font_bytes: 1000,
        ..Limits::default()
    });
    assert_eq!(
        code(small.register_fonts(vec![good])),
        ErrorCode::LimitExceeded
    );
}

#[test]
fn ops_that_are_not_built_refuse_with_their_feature_name() {
    let mut e = Engine::new(Limits::default());
    let (d, _) = e.open(pdf(&["x"]), None, false).unwrap();
    let feature = |r: Result<(), PdfError>| {
        let err = r.err().unwrap();
        (err.code, err.details.unwrap().feature.unwrap())
    };
    assert_eq!(
        feature(e.prepare_paragraph(&d, 0, vec![], None, false).map(|_| ())),
        (ErrorCode::InvalidArgument, "prepareParagraph".into())
    );
}
