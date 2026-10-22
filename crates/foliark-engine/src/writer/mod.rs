//! Native writer (spec §8.1, ADR 0002): a fresh, reachable-only, unencrypted, single-revision
//! file built from an allowlist. Content streams are re-emitted from tokens; later tasks
//! rewrite operator spans, clone Forms and re-encode images through the substitution API.
use crate::reader::{self, ReaderError};
use foliark_model::{ErrorCode, ErrorDetails, FoliarkError, Limits, ObjectId};
use pdf_writer::{Name, Null, Obj, Pdf, Ref, Str};
use std::collections::BTreeMap;
use std::ops::Range;

mod content;
mod graph;

/// Spec §12 `PdfError` (`FoliarkError` until the dedicated type lands), boxed to keep `Result`s small.
pub type PdfError = Box<FoliarkError>;

/// Span substitutions per content stream: decoded byte range and replacement operators.
pub(crate) type Spans = BTreeMap<ObjectId, Vec<(Range<usize>, Vec<u8>)>>;

pub(crate) fn err(code: ErrorCode, message: &str) -> PdfError {
    Box::new(FoliarkError {
        code,
        message: message.into(),
        details: None,
    })
}

pub(crate) fn err_obj(code: ErrorCode, message: &str, obj: ObjectId) -> PdfError {
    let mut e = err(code, message);
    e.details = Some(ErrorDetails {
        obj_ref: Some(format!("{} {} R", obj.num, obj.generation)),
        ..Default::default()
    });
    e
}

pub(crate) fn lim(e: ReaderError) -> PdfError {
    Box::new(FoliarkError::from(&e))
}

/// An owned PDF value. `Src` names an object of the input document (copied through the
/// allowlist when written); `Ref` is an output object.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Bool(bool),
    Int(i32),
    Real(f32),
    Str(Vec<u8>),
    Name(Vec<u8>),
    Arr(Vec<Val>),
    Dict(Vec<(Vec<u8>, Val)>),
    Src(ObjectId),
    Ref(Ref),
}

/// Replacement for a whole stream object: `dict` is filtered by the allowlist of the object's
/// class (no `/Length`, `/Filter` needed), `data` is the plain, unfiltered content.
#[derive(Debug, Clone, PartialEq)]
pub struct NewStream {
    pub dict: Vec<(String, Val)>,
    pub data: Vec<u8>,
}

pub struct Writer<'d> {
    doc: &'d reader::Document,
    limits: Limits,
    spans: Spans,
    repl: BTreeMap<ObjectId, NewStream>,
    appended: BTreeMap<usize, Vec<u8>>,
}

impl<'d> Writer<'d> {
    pub fn new(doc: &'d reader::Document, limits: Limits) -> Self {
        Self {
            doc,
            limits,
            spans: BTreeMap::new(),
            repl: BTreeMap::new(),
            appended: BTreeMap::new(),
        }
    }

    /// Replace every instruction of the decoded content stream `stream` that the byte `range`
    /// touches (ranges are the spans the reader reports) by `ops`, emitted once. Overlapping
    /// ranges and ranges that touch no instruction are refused at `write`.
    pub fn replace_span(&mut self, stream: ObjectId, range: Range<usize>, ops: &[u8]) -> &mut Self {
        self.spans
            .entry(stream)
            .or_default()
            .push((range, ops.to_vec()));
        self
    }

    /// Replace the whole object `obj` (a stream) by a new dictionary and plain data.
    pub fn replace_stream(&mut self, obj: ObjectId, new: NewStream) -> &mut Self {
        self.repl.insert(obj, new);
        self
    }

    /// Draw `ops` on top of page `page`: the existing content is wrapped in `q ... Q`.
    pub fn append_page_content(&mut self, page: usize, ops: &[u8]) -> Result<&mut Self, PdfError> {
        let count = self.doc.page_count();
        if page >= count {
            return Err(lim_page(page, count));
        }
        self.appended
            .entry(page)
            .or_default()
            .extend_from_slice(ops);
        Ok(self)
    }

    /// Write the file. Deterministic: same input and substitutions give the same bytes.
    pub fn write(&self) -> Result<Vec<u8>, PdfError> {
        for subs in self.spans.values() {
            let mut sorted: Vec<_> = subs.iter().map(|(r, _)| r.clone()).collect();
            sorted.sort_by_key(|r| r.start);
            if sorted.windows(2).any(|w| w[0].end > w[1].start) {
                return Err(err(
                    ErrorCode::InvalidArgument,
                    "overlapping span substitutions",
                ));
            }
        }
        let mut walk = graph::Walk::new(self.doc, &self.limits, &self.repl, &self.spans);
        let pages = walk.build(&self.appended)?;
        let max = u64::from(self.limits.max_output_bytes);
        let over = |actual: usize| {
            lim(crate::limits::exceeded(
                "maxOutputBytes",
                actual as u64,
                max,
            ))
        };
        let mut pdf = Pdf::new();
        pdf.catalog(Ref::new(1)).pages(Ref::new(2));
        pdf.pages(Ref::new(2))
            .kids(pages.iter().copied())
            .count(pages.len() as i32);
        for node in &walk.nodes {
            match &node.data {
                None => put(pdf.indirect(node.id), &node.val)?,
                Some(data) => {
                    let mut s = pdf.stream(node.id, data);
                    let Val::Dict(entries) = &node.val else {
                        return Err(err(ErrorCode::Internal, "stream without dictionary"));
                    };
                    for (k, v) in entries {
                        put(s.insert(Name(k)), v)?;
                    }
                }
            }
            if pdf.len() as u64 > max {
                return Err(over(pdf.len()));
            }
        }
        let out = pdf.finish();
        if out.len() as u64 > max {
            return Err(over(out.len()));
        }
        Ok(out)
    }
}

fn lim_page(page: usize, count: usize) -> PdfError {
    lim(ReaderError::PageOutOfRange { page, count })
}

fn put(obj: Obj<'_>, v: &Val) -> Result<(), PdfError> {
    match v {
        Val::Null => obj.primitive(Null),
        Val::Bool(b) => obj.primitive(*b),
        Val::Int(i) => obj.primitive(*i),
        Val::Real(f) => obj.primitive(*f),
        Val::Str(s) => obj.primitive(Str(s)),
        Val::Name(n) => obj.primitive(Name(n)),
        Val::Ref(r) => obj.primitive(*r),
        Val::Arr(items) => {
            let mut a = obj.array();
            for x in items {
                put(a.push(), x)?;
            }
        }
        Val::Dict(entries) => {
            let mut d = obj.dict();
            for (k, x) in entries {
                put(d.insert(Name(k)), x)?;
            }
        }
        Val::Src(_) => return Err(err(ErrorCode::Internal, "unresolved source reference")),
    }
    Ok(())
}
