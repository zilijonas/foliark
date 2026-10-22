//! Document-level sanitization (spec §8.2; NEW-41). Each fixture is exported through the
//! engine with no redaction; the output is judged by the independent oracles (secrets absent
//! from every object, stream and string; public page content intact) and the report counts are
//! compared with a count made on the input by pikepdf, not by the engine.
mod common;
use common::{fidelity_oracle, fixture, objects_oracle, python, repo, run};
use foliark_engine::Engine;
use foliark_model::{ExportMode, ExportPolicy, Limits, LossCode, SanitizedItem};

/// `mustRemove` and `mustKeepText` of a manifest fixture.
fn expectations(id: &str) -> (Vec<String>, Vec<String>) {
    let m: serde_json::Value = serde_json::from_slice(
        &std::fs::read(repo().join("tests/fixtures/manifest.json")).unwrap(),
    )
    .unwrap();
    let f = m["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == id)
        .unwrap();
    let list = |k: &str| {
        f["expected"][k]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect()
    };
    (list("mustRemove"), list("mustKeepText"))
}

fn export(e: &mut Engine, doc: &str) -> (foliark_model::ExportPlan, Vec<u8>) {
    let task = e.prepare_export(doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(e, task).unwrap();
    let out = e.take_export(doc, &plan.plan_id).unwrap();
    (plan, out)
}

struct Case {
    id: &'static str,
    path: &'static str,
    removed: &'static [(SanitizedItem, u32)],
    losses: &'static [LossCode],
}

use SanitizedItem::*;
const CASES: &[Case] = &[
    // Info (1) + the XMP stream (1).
    Case {
        id: "metadata-xmp",
        path: "hddn/metadata-xmp.pdf",
        removed: &[(Metadata, 2)],
        losses: &[LossCode::MetadataRemoved],
    },
    Case {
        id: "attachment",
        path: "hddn/attachment.pdf",
        removed: &[(Metadata, 1), (EmbeddedFiles, 1)],
        losses: &[LossCode::MetadataRemoved, LossCode::AttachmentsRemoved],
    },
    // OpenAction, the named JavaScript and one more action: three action dictionaries.
    Case {
        id: "actions-js",
        path: "hddn/actions-js.pdf",
        removed: &[(Metadata, 1), (Actions, 3), (Annotations, 1)],
        losses: &[
            LossCode::MetadataRemoved,
            LossCode::ScriptsRemoved,
            LossCode::LinksRemoved,
        ],
    },
    Case {
        id: "thumbnail",
        path: "hddn/thumbnail.pdf",
        removed: &[(Metadata, 1), (Thumbnails, 1)],
        losses: &[LossCode::MetadataRemoved],
    },
    Case {
        id: "outline-bookmark",
        path: "hddn/outline-bookmark.pdf",
        removed: &[(Metadata, 1), (Actions, 1), (Outlines, 1)],
        losses: &[
            LossCode::MetadataRemoved,
            LossCode::OutlinesRemoved,
            LossCode::ScriptsRemoved,
        ],
    },
    // StructTreeRoot + MarkInfo (the inline ActualText is checked separately).
    Case {
        id: "actualtext",
        path: "hddn/actualtext.pdf",
        removed: &[(Metadata, 1), (Structure, 2)],
        losses: &[LossCode::MetadataRemoved, LossCode::AccessibilityRemoved],
    },
    // Info + XMP, the attachment, and OpenAction + named JS + the AA action.
    Case {
        id: "leaks-metadata-attachment-js",
        path: "generated/leaks-metadata-attachment-js.pdf",
        removed: &[(Metadata, 2), (EmbeddedFiles, 1), (Actions, 3)],
        losses: &[
            LossCode::MetadataRemoved,
            LossCode::AttachmentsRemoved,
            LossCode::ScriptsRemoved,
        ],
    },
];

#[test]
fn payloads_are_absent_public_content_survives_and_the_counts_are_exact() {
    for c in CASES {
        let input = fixture(c.path);
        let (forbid, keep) = expectations(c.id);
        // Strings that are visible page text are public content here (redaction removes them,
        // sanitization must not): only the hidden payloads are forbidden.
        let page_text = {
            let d = foliark_engine::Document::open(input.clone(), None).unwrap();
            (0..d.page_count())
                .map(|p| d.extract_text(p, &Default::default()).unwrap().text)
                .collect::<String>()
        };
        let forbid: Vec<&str> = forbid
            .iter()
            .map(String::as_str)
            .filter(|f| !page_text.contains(f))
            .collect();
        assert!(
            !forbid.is_empty() || c.id == "thumbnail",
            "{}: no payload to forbid",
            c.id
        );
        let keep: Vec<&str> = keep.iter().map(String::as_str).collect();

        let mut e = Engine::new(Limits::default());
        let (doc, _) = e.open(input.clone(), None, false).unwrap();
        let (plan, out) = export(&mut e, &doc);

        let (ok, log) =
            objects_oracle("sanitize-document", &format!("{}.pdf", c.id), &out, &forbid);
        assert!(ok, "{}: objects.py: {log}", c.id);
        // Not vacuous: the oracle finds the payload in the input (the actualtext hex string is
        // covered by the content check below).
        let (found, _) = objects_oracle(
            "sanitize-document",
            &format!("{}.in.pdf", c.id),
            &input,
            &forbid,
        );
        assert!(
            !found || c.id == "actualtext",
            "{}: nothing found in the INPUT",
            c.id
        );
        if c.id == "actualtext" {
            // The input's ActualText makes extractors read SECRET_ACTUALTEXT over the glyphs
            // PUBLIC_GLYPHS; removing it changes the extracted words by design. What is drawn
            // is what must be extracted now.
            let d = foliark_engine::Document::open(out.clone(), None).unwrap();
            let text = d.extract_text(0, &Default::default()).unwrap().text;
            assert!(text.contains("PUBLIC_GLYPHS"), "{text}");
            for k in &keep {
                let (ok, log) = fidelity_oracle("sanitize-document", c.id, &out, &out, &[], &[k]);
                assert!(ok, "{}: {log}", c.id);
            }
        } else {
            let (ok, log) = fidelity_oracle("sanitize-document", c.id, &input, &out, &[], &keep);
            assert!(ok, "{}: fidelity.py: {log}", c.id);
        }

        let want: std::collections::BTreeMap<_, _> = c.removed.iter().copied().collect();
        assert_eq!(plan.report.removed, want, "{}: removed counts", c.id);
        let mut got: Vec<_> = plan.report.losses.iter().map(|l| l.code).collect();
        let mut codes = c.losses.to_vec();
        got.sort_by_key(|l| format!("{l:?}"));
        codes.sort_by_key(|l| format!("{l:?}"));
        assert_eq!(got, codes, "{}: loss codes", c.id);
        for l in &plan.report.losses {
            let item = match l.code {
                LossCode::MetadataRemoved => Metadata,
                LossCode::AttachmentsRemoved => EmbeddedFiles,
                LossCode::ScriptsRemoved => Actions,
                LossCode::OutlinesRemoved => Outlines,
                LossCode::LinksRemoved => Annotations,
                _ => Structure,
            };
            assert_eq!(Some(&l.count), want.get(&item), "{}: {:?}", c.id, l.code);
        }
    }
}

/// Content-stream bytes of every page of a PDF, decoded by pikepdf, counted for argv[2].
const COUNT: &str = concat!(
    "import sys, pikepdf\n",
    "pdf = pikepdf.open(sys.argv[1])\n",
    "n = 0\n",
    "for page in pdf.pages:\n",
    "    c = page.obj.Contents\n",
    "    for s in (list(c) if isinstance(c, pikepdf.Array) else [c]):\n",
    "        n += s.read_bytes().count(sys.argv[2].encode())\n",
    "print('COUNT', n)\n",
);

#[test]
fn accessibility_text_in_marked_content_is_stripped_but_the_tag_stays() {
    let input = fixture("hddn/actualtext.pdf");
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(input.clone(), None, false).unwrap();
    let (_, out) = export(&mut e, &doc);
    let count = |name: &str, bytes: &[u8], needle: &str| {
        let p = common::tmp("sanitize-document", name);
        std::fs::write(&p, bytes).unwrap();
        let (ok, log) = python(&["-c", COUNT, p.to_str().unwrap(), needle]);
        assert!(ok, "{log}");
        log.lines()
            .find_map(|l| l.strip_prefix("COUNT "))
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap()
    };
    assert_eq!(
        count("at.in.pdf", &input, "ActualText"),
        1,
        "the input carries it"
    );
    assert_eq!(count("at.out.pdf", &out, "ActualText"), 0);
    assert_eq!(count("at.out.pdf", &out, "/Span"), 1, "the tag is kept");
}
