//! Native export size (NEW-83): `cargo run --release -p foliark-engine --example export_size -- in.pdf out.pdf [keyword]`.
//! Redacts every hit of `keyword` (HDDN's keyword flow), exports natively and prints the output size.
use foliark_engine::{Engine, Step};
use foliark_model::{ChangeSet, ExportMode, ExportPolicy, Limits, Redaction, SearchOptions};

fn run<T>(e: &mut Engine, mut t: foliark_engine::BoxTask<T>) -> T {
    loop {
        if let Step::Done(v) = t.step(e, 1000).unwrap() {
            return v;
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let input = std::fs::read(&a[1]).unwrap();
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(input.clone(), None, false).unwrap();
    let mut hits = 0;
    if let Some(word) = a.get(3) {
        let t = e.search(&doc, word, SearchOptions::default());
        let reds: Vec<Redaction> = run(&mut e, t)
            .into_iter()
            .enumerate()
            .map(|(i, h)| Redaction::Text {
                id: format!("r{i}"),
                page_index: h.page_index,
                start: h.range.start,
                end: h.range.end,
                fill: None,
            })
            .collect();
        hits = reds.len();
        let t = e.apply_changes(
            &doc,
            ChangeSet {
                base_revision: e.revision(&doc).unwrap(),
                text_edits: None,
                redactions: Some(reds),
            },
        );
        assert!(run(&mut e, t).accepted);
    }
    let t = e.prepare_export(&doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(&mut e, t);
    let out = e.take_export(&doc, &plan.plan_id).unwrap();
    std::fs::write(&a[2], &out).unwrap();
    println!("in {} B, hits {hits}, out {} B", input.len(), out.len());
}
