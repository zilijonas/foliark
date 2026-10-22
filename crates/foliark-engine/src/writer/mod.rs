//! Native writer (spec §8.1, ADR 0002): a fresh, reachable-only, unencrypted, single-revision
//! file built from an allowlist. Content streams are re-emitted from tokens; later tasks
//! rewrite operator spans, clone Forms and re-encode images through the substitution API.
use crate::reader;
use foliark_model::{ErrorCode, Limits, ObjectId, PdfError};
use pdf_writer::{Name, Null, Obj, Pdf, Ref, Str};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

mod content;
pub use content::reemit;
pub mod fonts;
mod graph;
pub mod text;

/// Span substitutions per content stream: decoded byte range and replacement operators.
pub(crate) type Spans = BTreeMap<ObjectId, Vec<(Range<usize>, Vec<u8>)>>;

/// The wire `PdfError` carries a code and details only, so the message is documentation.
pub(crate) fn err(code: ErrorCode, _message: &str) -> PdfError {
    PdfError::new(code)
}

pub(crate) fn err_obj(code: ErrorCode, _message: &str, obj: ObjectId) -> PdfError {
    PdfError::at(code, obj)
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
    /// An object added with `Writer::add_object` (index); written once, only when reachable.
    New(usize),
}

/// An object the caller made (a registered font, its descriptor, ...): `data` is plain stream
/// content (compressed on write), `val` the dictionary or value. Not allowlist-filtered.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Extra {
    pub val: Val,
    pub data: Option<Vec<u8>>,
}

/// Replacement for a whole stream object: `dict` is filtered by the allowlist of the object's
/// class (no `/Length`, `/Filter` needed), `data` is the plain, unfiltered content.
#[derive(Debug, Clone, PartialEq)]
pub struct NewStream {
    pub dict: Vec<(String, Val)>,
    pub data: Vec<u8>,
}

/// XObject resources added (name, object) and dropped (name).
pub(crate) type XObjectEdits = (Vec<(Vec<u8>, Val)>, Vec<Vec<u8>>);

/// Where a clone of a source object goes and what a Form's resources gain or lose. Clones get
/// ids no input object has (generation 65535 is a free slot, never a live object).
#[derive(Debug, Default)]
pub(crate) struct Redirects {
    /// Clone id -> the source object it copies (dictionary and stream data).
    pub aliases: BTreeMap<ObjectId, ObjectId>,
    /// A page's `/Contents` entry `source` is written as the clone instead.
    pub page_streams: BTreeMap<(usize, ObjectId), ObjectId>,
    /// Per Form (source or clone id): XObject resources added and dropped.
    pub form_xobjects: BTreeMap<ObjectId, XObjectEdits>,
}

pub struct Writer<'d> {
    doc: &'d reader::Document,
    limits: Limits,
    spans: Spans,
    repl: BTreeMap<ObjectId, NewStream>,
    appended: BTreeMap<usize, Vec<u8>>,
    extras: Vec<Extra>,
    page_fonts: PageFonts,
    page_xobjects: PageFonts,
    dropped_xobjects: BTreeMap<usize, Vec<Vec<u8>>>,
    to_unicode: BTreeMap<ObjectId, Vec<u8>>,
    /// Registered font ids already written (`fonts::embed_registered_font`).
    embedded: BTreeSet<String>,
    redirects: Redirects,
}

/// Resources added per page: (resource name, object).
pub(crate) type PageFonts = BTreeMap<usize, Vec<(Vec<u8>, Val)>>;

impl<'d> Writer<'d> {
    pub fn new(doc: &'d reader::Document, limits: Limits) -> Self {
        Self {
            doc,
            limits,
            spans: BTreeMap::new(),
            repl: BTreeMap::new(),
            appended: BTreeMap::new(),
            extras: Vec::new(),
            page_fonts: BTreeMap::new(),
            page_xobjects: BTreeMap::new(),
            dropped_xobjects: BTreeMap::new(),
            to_unicode: BTreeMap::new(),
            embedded: BTreeSet::new(),
            redirects: Redirects::default(),
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
            return Err(PdfError::page_out_of_range(page));
        }
        self.appended
            .entry(page)
            .or_default()
            .extend_from_slice(ops);
        Ok(self)
    }

    /// Add an output object (`data` = plain stream content). The returned `Val::New` may be
    /// used inside other added objects and in `add_page_font`; it is written once, only if
    /// something reaches it.
    pub fn add_object(&mut self, val: Val, data: Option<Vec<u8>>) -> Val {
        self.extras.push(Extra { val, data });
        Val::New(self.extras.len() - 1)
    }

    /// Make `font` (from `add_object`) usable as `/name Tf` on page `page`. A name the page
    /// already has is refused at `write`.
    pub fn add_page_font(
        &mut self,
        page: usize,
        name: &str,
        font: Val,
    ) -> Result<&mut Self, PdfError> {
        if page >= self.doc.page_count() {
            return Err(PdfError::page_out_of_range(page));
        }
        self.page_fonts
            .entry(page)
            .or_default()
            .push((name.as_bytes().to_vec(), font));
        Ok(self)
    }

    /// Make `xobject` (from `add_object`) usable as `/name Do` on page `page`. A name the page
    /// already has is refused at `write`. Only page content: a Form's own resources are not
    /// reachable here.
    pub fn add_page_xobject(
        &mut self,
        page: usize,
        name: &str,
        xobject: Val,
    ) -> Result<&mut Self, PdfError> {
        if page >= self.doc.page_count() {
            return Err(PdfError::page_out_of_range(page));
        }
        self.page_xobjects
            .entry(page)
            .or_default()
            .push((name.as_bytes().to_vec(), xobject));
        Ok(self)
    }

    /// Take the `/name` entry out of page `page`'s XObject resources, so an XObject no content
    /// draws any more (an image whose every occurrence was cloned) is not reachable from it.
    pub fn drop_page_xobject(&mut self, page: usize, name: &str) -> Result<&mut Self, PdfError> {
        if page >= self.doc.page_count() {
            return Err(PdfError::page_out_of_range(page));
        }
        self.dropped_xobjects
            .entry(page)
            .or_default()
            .push(name.as_bytes().to_vec());
        Ok(self)
    }

    /// Make `clone` a separate output object that copies `source` (dictionary, stream data).
    /// Spans, replacements and resource edits keyed by `clone` apply to the copy only. Refused
    /// when `clone` is already used. Reference it as `Val::Src(clone)` or through
    /// `redirect_page_content`.
    pub fn alias(&mut self, clone: ObjectId, source: ObjectId) -> Result<&mut Self, PdfError> {
        if self.redirects.aliases.insert(clone, source).is_some() {
            return Err(err(ErrorCode::InvalidArgument, "clone id in use"));
        }
        Ok(self)
    }

    /// Whether `id` was made a clone by `alias`.
    pub fn is_clone(&self, id: ObjectId) -> bool {
        self.redirects.aliases.contains_key(&id)
    }

    /// Page `page` draws the clone `clone` where its `/Contents` names the stream `source`.
    pub fn redirect_page_content(
        &mut self,
        page: usize,
        source: ObjectId,
        clone: ObjectId,
    ) -> Result<&mut Self, PdfError> {
        if page >= self.doc.page_count() {
            return Err(PdfError::page_out_of_range(page));
        }
        self.redirects.page_streams.insert((page, source), clone);
        Ok(self)
    }

    /// Make `xobject` usable as `/name Do` inside Form `form` (a source Form or a clone). The
    /// Form must have `/Resources` of its own; a name it already has is refused at `write`.
    pub fn add_form_xobject(&mut self, form: ObjectId, name: &str, xobject: Val) -> &mut Self {
        let entry = self.redirects.form_xobjects.entry(form).or_default();
        entry.0.push((name.as_bytes().to_vec(), xobject));
        self
    }

    /// Take `/name` out of Form `form`'s XObject resources (see `drop_page_xobject`).
    pub fn drop_form_xobject(&mut self, form: ObjectId, name: &str) -> &mut Self {
        let entry = self.redirects.form_xobjects.entry(form).or_default();
        entry.1.push(name.as_bytes().to_vec());
        self
    }

    /// Use `cmap` (plain CMap text) as the ToUnicode of the source font dictionary `font`.
    pub fn set_to_unicode(&mut self, font: ObjectId, cmap: Vec<u8>) -> &mut Self {
        self.to_unicode.insert(font, cmap);
        self
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
        let mut walk = graph::Walk::new(
            self.doc,
            &self.limits,
            &self.repl,
            &self.spans,
            (&self.extras, &self.to_unicode),
            &self.redirects,
        );
        let pages = walk.build(
            &self.appended,
            [&self.page_fonts, &self.page_xobjects],
            &self.dropped_xobjects,
        )?;
        let max = u64::from(self.limits.max_output_bytes);
        let over = |actual: usize| PdfError::limit("maxOutputBytes", actual as u64, max);
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
        Val::Src(_) | Val::New(_) => {
            return Err(err(ErrorCode::Internal, "unresolved reference"));
        }
    }
    Ok(())
}
