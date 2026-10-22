//! Document-level rows of spec §8.2: what the allowlist writer will not carry over from the
//! file, counted by reachable object so `ExportReport.removed` and the loss codes are exact.
//! Reads dictionaries only; nothing is executed and no document text leaves this module.
//!
//! Not counted here (removed all the same): inline `ActualText`/`Alt`/`E` in content-stream
//! `BDC` property lists (the content re-emitter strips them; `structure` counts the catalog's
//! `StructTreeRoot` and `MarkInfo` only), `ViewerPreferences`, `PageMode`, page `/Trans`/`/Dur`.
use crate::reader::Document;
use foliark_model::{ErrorCode, Loss, LossCode, PdfError, SanitizedItem};
use hayro::hayro_syntax::object::{Dict, MaybeRef, Name, Object, ObjectIdentifier};
use std::collections::{BTreeMap, HashSet};

/// Action types: every one of them is removed (JavaScript, URI, Launch, GoToR, SubmitForm
/// and the rest), none is kept even when harmless.
const ACTIONS: &[&str] = &[
    "GoTo",
    "GoToR",
    "GoToE",
    "GoTo3DView",
    "Launch",
    "Thread",
    "URI",
    "Sound",
    "Movie",
    "Hide",
    "Named",
    "SubmitForm",
    "ResetForm",
    "ImportData",
    "JavaScript",
    "SetOCGState",
    "Rendition",
    "Trans",
];

#[derive(Debug, Default, Clone, PartialEq)]
pub struct DocumentRemovals {
    pub removed: BTreeMap<SanitizedItem, u32>,
    /// One per feature class removed, with the number of items.
    pub losses: Vec<Loss>,
    /// Signature values (or a bare `/Perms`): `signedInput: refuse` stops on these.
    pub signatures: u32,
}

#[derive(Default)]
struct Counts {
    metadata: u32,
    embedded: u32,
    actions: u32,
    thumbnails: u32,
    outline_items: u32,
    signatures: u32,
}

struct Walk<'a> {
    pdf: &'a hayro::hayro_syntax::Pdf,
    seen: HashSet<ObjectIdentifier>,
    todo: Vec<ObjectIdentifier>,
    c: Counts,
}

impl Walk<'_> {
    fn dict(&mut self, d: &Dict<'_>) {
        let c = &mut self.c;
        if d.get::<Name<'_>>("S")
            .is_some_and(|n| ACTIONS.contains(&n.as_str()))
        {
            c.actions += 1;
        }
        if d.get::<Name<'_>>("Type")
            .is_some_and(|n| n.as_str() == "Metadata")
        {
            c.metadata += 1;
        }
        c.metadata += u32::from(d.contains_key("PieceInfo"));
        c.thumbnails += u32::from(d.contains_key("Thumb"));
        c.signatures += u32::from(d.contains_key("ByteRange") && d.contains_key("Contents"));
        c.outline_items += u32::from(d.contains_key("Title") && d.contains_key("Parent"));
        if let Some(ef) = d.get::<Dict<'_>>("EF") {
            c.embedded += ef.keys().count() as u32;
        }
    }

    fn visit(&mut self, o: &Object<'_>, depth: usize) {
        match o {
            Object::Dict(d) => self.entries(d, depth),
            Object::Stream(s) => self.entries(s.dict(), depth),
            Object::Array(a) => {
                for item in a.raw_iter() {
                    self.child(item, depth);
                }
            }
            _ => {}
        }
    }

    fn entries(&mut self, d: &Dict<'_>, depth: usize) {
        self.dict(d);
        for (_, v) in d.entries() {
            self.child(v, depth);
        }
    }

    fn child(&mut self, v: MaybeRef<Object<'_>>, depth: usize) {
        match v {
            MaybeRef::Ref(r) => {
                let id = ObjectIdentifier::from(r);
                if self.seen.insert(id) {
                    self.todo.push(id);
                }
            }
            // Direct nesting is bounded by the parser; the cap only stops a pathological file.
            MaybeRef::NotRef(o) if depth < 64 => self.visit(&o, depth + 1),
            MaybeRef::NotRef(_) => {}
        }
    }
}

/// Count what the allowlist writer drops from `doc`'s catalog, trailer and everything
/// reachable from them. Errors: `MALFORMED_PDF` when the catalog is not a dictionary.
pub fn scan(doc: &Document) -> Result<DocumentRemovals, PdfError> {
    let pdf = doc.pdf();
    let root = pdf.xref().root_id();
    let catalog = pdf
        .xref()
        .get::<Dict<'_>>(root)
        .ok_or(PdfError::new(ErrorCode::MalformedPdf))?;
    let mut w = Walk {
        pdf,
        seen: HashSet::from([root]),
        todo: Vec::new(),
        c: Counts::default(),
    };
    w.entries(&catalog, 0);
    while let Some(id) = w.todo.pop() {
        if let Some(o) = w.pdf.xref().get::<Object<'_>>(id) {
            w.visit(&o, 0);
        }
    }
    let c = w.c;

    let has = |k: &str| catalog.contains_key(k);
    let names = catalog.get::<Dict<'_>>("Names");
    let in_names = |f: &dyn Fn(&str) -> bool| {
        names
            .as_ref()
            .is_some_and(|n| n.keys().any(|k| f(k.as_str())))
    };
    let intents = catalog
        .get::<hayro::hayro_syntax::object::Array<'_>>("OutputIntents")
        .map_or(0, |a| a.raw_iter().count() as u32);
    let nav = u32::from(has("Dests"))
        + u32::from(has("Threads"))
        + u32::from(in_names(&|k| k != "JavaScript" && k != "EmbeddedFiles"));
    let outlines = if c.outline_items > 0 {
        c.outline_items + nav
    } else {
        u32::from(has("Outlines")) + nav
    };
    let info = u32::from(*pdf.metadata() != Default::default());
    let signatures = if c.signatures > 0 {
        c.signatures
    } else {
        u32::from(has("Perms"))
    };

    let mut removed = BTreeMap::new();
    let mut losses = Vec::new();
    let mut put = |item: SanitizedItem, code: Option<LossCode>, n: u32| {
        if n > 0 {
            removed.insert(item, n);
            losses.extend(code.map(|code| Loss {
                code,
                count: n,
                pages: None,
            }));
        }
    };
    use LossCode as L;
    use SanitizedItem as S;
    put(S::Metadata, Some(L::MetadataRemoved), c.metadata + info);
    put(
        S::EmbeddedFiles,
        Some(L::AttachmentsRemoved),
        c.embedded + u32::from(has("Collection")),
    );
    put(S::Actions, Some(L::ScriptsRemoved), c.actions);
    put(S::Thumbnails, None, c.thumbnails);
    put(
        S::Structure,
        Some(L::AccessibilityRemoved),
        u32::from(has("StructTreeRoot")) + u32::from(has("MarkInfo")),
    );
    put(S::Outlines, Some(L::OutlinesRemoved), outlines);
    put(
        S::PageLabels,
        Some(L::PageLabelsRemoved),
        u32::from(has("PageLabels")),
    );
    for (code, n) in [
        (L::PdfaClaimRemoved, intents),
        (L::EncryptionRemoved, u32::from(pdf.is_encrypted())),
        (L::SignaturesInvalidated, signatures),
    ] {
        if n > 0 {
            losses.push(Loss {
                code,
                count: n,
                pages: None,
            });
        }
    }
    Ok(DocumentRemovals {
        removed,
        losses,
        signatures,
    })
}
