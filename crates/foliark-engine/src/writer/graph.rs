//! Allowlisted graph walk (spec §8.1, ADR 0002). Objects are reached only through allowlisted
//! keys, each with a class; a class lists the keys it emits (and the class of each value), so
//! everything else is dropped by construction. Output objects get fresh ids in discovery order.
//!
//! Rule for unknown things: a reachable object whose class cannot be decided (an XObject
//! that is neither Image nor Form, a font Subtype we do not know, a stream where a plain
//! value belongs) is a typed `MALFORMED_PDF` refusal, never a silent skip. Direct values are
//! copied; indirect values in plain positions are inlined; dangling references are errors.
use super::{
    Extra, Link, LinkTarget, NewStream, Owner, PageFonts, Redirects, Spans, Val, content, err,
    err_obj,
};
use crate::limits::{self, Budget};
use crate::reader::{self, object_id};
use foliark_model::{ErrorCode, Limits, ObjectId, PdfError};
use hayro::hayro_syntax::Filter;
use hayro::hayro_syntax::object::dict::keys::{FILTER, PARENT};
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, ObjectIdentifier, Stream};
use pdf_writer::Ref;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Write;
use std::ops::Deref;

type Entries = Vec<(Vec<u8>, Val)>;
const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Class {
    Resources,
    Group,
    XObject,
    Image,
    Form,
    Font,
    SimpleFont,
    Type3Font,
    Type0Font,
    CidFont,
    CidSystemInfo,
    FontDescriptor,
    FontFile,
    CMap,
    Encoding,
    /// A bare content stream (page contents, Type3 glyph procedures): no dictionary keys.
    Content,
    ExtGState,
    SoftMask,
    ColorSpace,
    IccStream,
    Pattern,
    Tiling,
    ShadingPattern,
    Shading,
    Function,
    PropList,
    DecodeParms,
    /// Opaque data (JBIG2 globals, CIDToGIDMap, Indexed lookup).
    Data,
    /// Preserve mode only: any object no class above claims. Every key is kept, a stream keeps
    /// its bytes.
    Raw,
}

/// How a value is copied.
#[derive(Clone, Copy)]
pub(crate) enum V {
    /// Any direct value; references are inlined, a stream is refused.
    Plain,
    /// An object of the class (reference kept as a new indirect object); names pass through.
    One(Class),
    /// One object or an array of them.
    Many(Class),
    /// A dictionary name -> object of the class.
    Map(Class),
    /// Preserve mode: copy the value as is; every reference becomes an output object, classified
    /// by what it points to.
    Any,
    /// An array with per-position specs; further items are plain.
    Tuple(&'static [V]),
}

use Class as C;
use V::{Many, Map, One, Plain};

impl Class {
    fn keys(self) -> &'static [(&'static str, V)] {
        match self {
            C::Resources => &[
                ("Font", Map(C::Font)),
                ("XObject", Map(C::XObject)),
                ("ExtGState", Map(C::ExtGState)),
                ("ColorSpace", Map(C::ColorSpace)),
                ("Pattern", Map(C::Pattern)),
                ("Shading", Map(C::Shading)),
                ("Properties", Map(C::PropList)),
                ("ProcSet", Plain),
            ],
            C::Group => &[
                ("Type", Plain),
                ("S", Plain),
                ("CS", One(C::ColorSpace)),
                ("I", Plain),
                ("K", Plain),
            ],
            C::Image => &[
                ("Type", Plain),
                ("Subtype", Plain),
                ("Width", Plain),
                ("Height", Plain),
                ("ColorSpace", One(C::ColorSpace)),
                ("BitsPerComponent", Plain),
                ("Decode", Plain),
                ("ImageMask", Plain),
                ("Mask", One(C::Image)),
                ("SMask", One(C::Image)),
                ("SMaskInData", Plain),
                ("Interpolate", Plain),
                ("Intent", Plain),
                // Not in spec 8.1 but needed for a soft mask to render (premultiplied matte).
                ("Matte", Plain),
            ],
            C::Form => &[
                ("Type", Plain),
                ("Subtype", Plain),
                ("FormType", Plain),
                ("BBox", Plain),
                ("Matrix", Plain),
                ("Resources", One(C::Resources)),
                ("Group", One(C::Group)),
            ],
            C::SimpleFont => &[
                ("Type", Plain),
                ("Subtype", Plain),
                ("BaseFont", Plain),
                ("FirstChar", Plain),
                ("LastChar", Plain),
                ("Widths", Plain),
                ("FontDescriptor", One(C::FontDescriptor)),
                ("Encoding", One(C::Encoding)),
                ("ToUnicode", One(C::CMap)),
            ],
            C::Type3Font => &[
                ("Type", Plain),
                ("Subtype", Plain),
                ("Name", Plain),
                ("FontBBox", Plain),
                ("FontMatrix", Plain),
                ("CharProcs", Map(C::Content)),
                ("Encoding", One(C::Encoding)),
                ("FirstChar", Plain),
                ("LastChar", Plain),
                ("Widths", Plain),
                ("FontDescriptor", One(C::FontDescriptor)),
                ("Resources", One(C::Resources)),
                ("ToUnicode", One(C::CMap)),
            ],
            C::Type0Font => &[
                ("Type", Plain),
                ("Subtype", Plain),
                ("BaseFont", Plain),
                ("Encoding", One(C::CMap)),
                ("DescendantFonts", Many(C::Font)),
                ("ToUnicode", One(C::CMap)),
            ],
            C::CidFont => &[
                ("Type", Plain),
                ("Subtype", Plain),
                ("BaseFont", Plain),
                ("CIDSystemInfo", One(C::CidSystemInfo)),
                ("FontDescriptor", One(C::FontDescriptor)),
                ("DW", Plain),
                ("W", Plain),
                ("DW2", Plain),
                ("W2", Plain),
                ("CIDToGIDMap", One(C::Data)),
            ],
            C::CidSystemInfo => &[
                ("Registry", Plain),
                ("Ordering", Plain),
                ("Supplement", Plain),
            ],
            // CharSet and CIDSet are dropped on purpose: optional, and they list glyphs.
            C::FontDescriptor => &[
                ("Type", Plain),
                ("FontName", Plain),
                ("FontFamily", Plain),
                ("FontStretch", Plain),
                ("FontWeight", Plain),
                ("Flags", Plain),
                ("FontBBox", Plain),
                ("ItalicAngle", Plain),
                ("Ascent", Plain),
                ("Descent", Plain),
                ("Leading", Plain),
                ("CapHeight", Plain),
                ("XHeight", Plain),
                ("StemV", Plain),
                ("StemH", Plain),
                ("AvgWidth", Plain),
                ("MaxWidth", Plain),
                ("MissingWidth", Plain),
                ("FontFile", One(C::FontFile)),
                ("FontFile2", One(C::FontFile)),
                ("FontFile3", One(C::FontFile)),
            ],
            C::FontFile => &[
                ("Subtype", Plain),
                ("Length1", Plain),
                ("Length2", Plain),
                ("Length3", Plain),
            ],
            C::CMap => &[
                ("Type", Plain),
                ("CMapName", Plain),
                ("CIDSystemInfo", One(C::CidSystemInfo)),
                ("WMode", Plain),
                ("UseCMap", One(C::CMap)),
            ],
            C::Encoding => &[
                ("Type", Plain),
                ("BaseEncoding", Plain),
                ("Differences", Plain),
            ],
            C::ExtGState => &[
                ("Type", Plain),
                ("LW", Plain),
                ("LC", Plain),
                ("LJ", Plain),
                ("ML", Plain),
                ("D", Plain),
                ("RI", Plain),
                ("OP", Plain),
                ("op", Plain),
                ("OPM", Plain),
                ("Font", V::Tuple(&[One(C::Font), Plain])),
                ("BG", Many(C::Function)),
                ("BG2", Many(C::Function)),
                ("UCR", Many(C::Function)),
                ("UCR2", Many(C::Function)),
                ("TR", Many(C::Function)),
                ("TR2", Many(C::Function)),
                ("FL", Plain),
                ("SM", Plain),
                ("SA", Plain),
                ("BM", Plain),
                ("SMask", One(C::SoftMask)),
                ("CA", Plain),
                ("ca", Plain),
                ("AIS", Plain),
                ("TK", Plain),
            ],
            C::SoftMask => &[
                ("Type", Plain),
                ("S", Plain),
                ("G", One(C::Form)),
                ("BC", Plain),
                ("TR", Many(C::Function)),
            ],
            C::IccStream => &[
                ("N", Plain),
                ("Alternate", One(C::ColorSpace)),
                ("Range", Plain),
            ],
            C::Tiling => &[
                ("Type", Plain),
                ("PatternType", Plain),
                ("PaintType", Plain),
                ("TilingType", Plain),
                ("BBox", Plain),
                ("XStep", Plain),
                ("YStep", Plain),
                ("Resources", One(C::Resources)),
                ("Matrix", Plain),
            ],
            C::ShadingPattern => &[
                ("Type", Plain),
                ("PatternType", Plain),
                ("Shading", One(C::Shading)),
                ("Matrix", Plain),
                ("ExtGState", One(C::ExtGState)),
            ],
            C::Shading => &[
                ("ShadingType", Plain),
                ("ColorSpace", One(C::ColorSpace)),
                ("Background", Plain),
                ("BBox", Plain),
                ("AntiAlias", Plain),
                ("Function", Many(C::Function)),
                ("Coords", Plain),
                ("Domain", Plain),
                ("Extend", Plain),
                ("Matrix", Plain),
                ("BitsPerCoordinate", Plain),
                ("BitsPerComponent", Plain),
                ("BitsPerFlag", Plain),
                ("Decode", Plain),
                ("VerticesPerRow", Plain),
            ],
            C::Function => &[
                ("FunctionType", Plain),
                ("Domain", Plain),
                ("Range", Plain),
                ("Size", Plain),
                ("BitsPerSample", Plain),
                ("Order", Plain),
                ("Encode", Plain),
                ("Decode", Plain),
                ("C0", Plain),
                ("C1", Plain),
                ("N", Plain),
                ("Functions", Many(C::Function)),
                ("Bounds", Plain),
            ],
            // Marked-content property lists: layer names only. ActualText, Alt, E and the
            // like are never listed (spec 8.2 removes them; OC flattening is a later task).
            C::PropList => &[
                ("Type", Plain),
                ("Name", Plain),
                ("Intent", Plain),
                ("OCGs", Many(C::PropList)),
                ("P", Plain),
                ("VE", Plain),
                ("MCID", Plain),
            ],
            C::DecodeParms => &[
                ("Predictor", Plain),
                ("Colors", Plain),
                ("BitsPerComponent", Plain),
                ("Columns", Plain),
                ("EarlyChange", Plain),
                ("K", Plain),
                ("EndOfLine", Plain),
                ("EncodedByteAlign", Plain),
                ("Rows", Plain),
                ("EndOfBlock", Plain),
                ("BlackIs1", Plain),
                ("DamagedRowsBeforeError", Plain),
                ("JBIG2Globals", One(C::Data)),
                ("ColorTransform", Plain),
            ],
            C::XObject | C::Font | C::Pattern | C::ColorSpace | C::Content | C::Data | C::Raw => {
                &[]
            }
        }
    }

    fn is_content(self) -> bool {
        matches!(self, C::Form | C::Tiling | C::Content)
    }

    fn allows_data_stream(self) -> bool {
        matches!(
            self,
            C::Image
                | C::FontFile
                | C::CMap
                | C::IccStream
                | C::Data
                | C::Shading
                | C::Function
                | C::Raw
        )
    }
}

fn get<'v>(entries: &'v Entries, key: &str) -> Option<&'v Val> {
    entries
        .iter()
        .find(|(k, _)| k == key.as_bytes())
        .map(|(_, v)| v)
}

/// Narrow a polymorphic class by the object's `/Subtype` or `/PatternType`.
fn refine(class: Class, val: &Val, obj: ObjectId) -> Result<Class, PdfError> {
    let Val::Dict(d) = val else { return Ok(class) };
    let name = |k: &str| match get(d, k) {
        Some(Val::Name(n)) => Some(n.as_slice()),
        _ => None,
    };
    let refined = match class {
        C::XObject => match name("Subtype") {
            Some(b"Image") => Some(C::Image),
            Some(b"Form") => Some(C::Form),
            _ => None,
        },
        C::Font => match name("Subtype") {
            Some(b"Type1" | b"MMType1" | b"TrueType") => Some(C::SimpleFont),
            Some(b"Type3") => Some(C::Type3Font),
            Some(b"Type0") => Some(C::Type0Font),
            Some(b"CIDFontType0" | b"CIDFontType2") => Some(C::CidFont),
            _ => None,
        },
        C::Pattern => match get(d, "PatternType") {
            Some(Val::Int(1)) => Some(C::Tiling),
            Some(Val::Int(2)) => Some(C::ShadingPattern),
            _ => None,
        },
        _ => return Ok(class),
    };
    refined.ok_or_else(|| err_obj(ErrorCode::MalformedPdf, "unclassifiable object", obj))
}

/// See `Walk::kind`.
enum Kind {
    Missing,
    PageTree,
    Is(Class),
}

pub(crate) struct Node {
    pub id: Ref,
    pub val: Val,
    /// Stream data, already encoded; `val` is then the stream dictionary.
    pub data: Option<Vec<u8>>,
}

pub(crate) struct Walk<'a> {
    doc: &'a reader::Document,
    limits: &'a Limits,
    repl: &'a BTreeMap<ObjectId, NewStream>,
    spans: &'a Spans,
    extras: &'a [Extra],
    to_unicode: &'a BTreeMap<ObjectId, Vec<u8>>,
    redirects: &'a Redirects,
    extra_refs: HashMap<usize, Ref>,
    budget: Budget,
    pub nodes: Vec<Node>,
    memo: HashMap<(ObjectId, Class), Ref>,
    queue: VecDeque<(ObjectId, Class, Ref)>,
    next: i32,
    bytes: usize,
    /// Optional content was flattened by the caller (`Writer::resolve_layers`).
    pub layers_resolved: bool,
    /// Encoded stream bytes that replace the source's (`Writer::replace_stream_data`).
    pub encoded: &'a BTreeMap<ObjectId, Vec<u8>>,
    /// Preserve mode (spec 8.3, `Writer::preserve`): nothing is dropped.
    pub preserve: bool,
    /// Store identical objects once (`Writer::keep_duplicates` turns it off).
    pub dedupe: bool,
    /// Hyperlinks to write into their pages' `/Annots` (`Writer::set_links`).
    pub links: &'a [Link],
    /// Page and page tree objects of the input -> their output reference.
    page_refs: HashMap<ObjectId, Ref>,
    pages_ref: Option<Ref>,
    /// The output `/Info` object, in preserve mode when the input has one.
    pub info: Option<Ref>,
}

static NO_ENCODED: BTreeMap<ObjectId, Vec<u8>> = BTreeMap::new();

fn name_val(n: &[u8]) -> Val {
    Val::Name(n.to_vec())
}

pub(super) fn deflate(data: &[u8]) -> Vec<u8> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    z.write_all(data).expect("write to Vec");
    z.finish().expect("write to Vec")
}

impl<'a> Walk<'a> {
    pub fn new(
        doc: &'a reader::Document,
        limits: &'a Limits,
        repl: &'a BTreeMap<ObjectId, NewStream>,
        spans: &'a Spans,
        (extras, to_unicode): (&'a [Extra], &'a BTreeMap<ObjectId, Vec<u8>>),
        redirects: &'a Redirects,
    ) -> Self {
        Self {
            doc,
            limits,
            repl,
            spans,
            extras,
            to_unicode,
            redirects,
            extra_refs: HashMap::new(),
            budget: Budget::new(limits),
            nodes: Vec::new(),
            memo: HashMap::new(),
            queue: VecDeque::new(),
            next: 1,
            bytes: 0,
            layers_resolved: false,
            encoded: &NO_ENCODED,
            preserve: false,
            dedupe: true,
            links: &[],
            page_refs: HashMap::new(),
            pages_ref: None,
            info: None,
        }
    }

    pub fn alloc(&mut self) -> Result<Ref, PdfError> {
        limits::check(
            "maxObjects",
            self.next as u64,
            self.limits.max_objects.into(),
        )?;
        let r = Ref::new(self.next);
        self.next += 1;
        Ok(r)
    }

    /// Account for a finished object before it is kept (the output budget is checked while
    /// building, not after).
    pub fn push(&mut self, node: Node) -> Result<(), PdfError> {
        self.bytes += node.data.as_ref().map_or(0, Vec::len) + 64;
        limits::check_output(self.bytes as u64, !self.preserve, self.limits)?;
        self.nodes.push(node);
        Ok(())
    }

    /// A new flate-compressed stream object holding `plain`.
    pub fn plain_stream(&mut self, plain: &[u8]) -> Result<Val, PdfError> {
        let id = self.alloc()?;
        let val = Val::Dict(vec![(b"Filter".to_vec(), name_val(b"FlateDecode"))]);
        self.push(Node {
            id,
            val,
            data: Some(deflate(plain)),
        })?;
        Ok(Val::Ref(id))
    }

    // ---- loading ---------------------------------------------------------------------

    pub fn load(&self, id: ObjectId) -> Result<(Val, Option<Stream<'a>>), PdfError> {
        let ident = ObjectIdentifier::new(id.num as i32, i32::from(id.generation));
        let obj = self
            .doc
            .pdf()
            .xref()
            .get::<Object<'a>>(ident)
            .ok_or_else(|| err_obj(ErrorCode::MalformedPdf, "dangling reference", id))?;
        let val = self.to_val(&obj, 0)?;
        Ok((
            val,
            if let Object::Stream(s) = obj {
                Some(s)
            } else {
                None
            },
        ))
    }

    fn maybe(&self, m: MaybeRef<Object<'a>>, depth: usize) -> Result<Val, PdfError> {
        match m {
            MaybeRef::Ref(r) => Ok(Val::Src(object_id(r))),
            MaybeRef::NotRef(o) => self.to_val(&o, depth + 1),
        }
    }

    /// Shallow-to-deep conversion of a parsed object; references stay `Src`.
    fn to_val(&self, o: &Object<'a>, depth: usize) -> Result<Val, PdfError> {
        if depth > MAX_DEPTH {
            return Err(err(ErrorCode::MalformedPdf, "object nesting too deep"));
        }
        Ok(match o {
            Object::Null(_) => Val::Null,
            Object::Boolean(b) => Val::Bool(*b),
            Object::Number(n) => {
                let f = n.as_f64();
                if f.fract() == 0.0 && f.abs() <= f64::from(i32::MAX) {
                    Val::Int(f as i32)
                } else {
                    Val::Real(f as f32)
                }
            }
            Object::String(s) => Val::Str(s.as_bytes().to_vec()),
            Object::Name(n) => name_val(n.deref()),
            Object::Array(a) => Val::Arr(
                a.raw_iter()
                    .map(|m| self.maybe(m, depth))
                    .collect::<Result<_, _>>()?,
            ),
            Object::Dict(d) => self.dict_val(d, depth)?,
            Object::Stream(s) => self.dict_val(s.dict(), depth)?,
        })
    }

    fn dict_val(&self, d: &Dict<'a>, depth: usize) -> Result<Val, PdfError> {
        Ok(Val::Dict(
            d.entries()
                .map(|(k, m)| Ok((k.deref().to_vec(), self.maybe(m, depth)?)))
                .collect::<Result<_, PdfError>>()?,
        ))
    }

    // ---- emission --------------------------------------------------------------------

    /// The output reference for `id` as `class`, queued for emission on first use.
    fn object(&mut self, id: ObjectId, class: Class) -> Result<Val, PdfError> {
        let redirects = self.redirects;
        if let Some(v) = redirects.superseded.get(&id) {
            return self.resolve_new(v, 0);
        }
        let class = match (self.preserve, class) {
            (false, _) | (_, C::Content | C::Form | C::Tiling) => class,
            (true, C::Font | C::XObject | C::Pattern) => match self.kind(id)? {
                Kind::Is(c) => c,
                _ => C::Raw,
            },
            (true, _) => C::Raw,
        };
        if let Some(r) = self.memo.get(&(id, class)) {
            return Ok(Val::Ref(*r));
        }
        let r = self.alloc()?;
        self.memo.insert((id, class), r);
        self.queue.push_back((id, class, r));
        Ok(Val::Ref(r))
    }

    /// Preserve mode: the output value for a reference to whatever `id` is. Pages and page tree
    /// nodes name the rewritten page tree, objects the file lacks are null.
    fn any(&mut self, id: ObjectId) -> Result<Val, PdfError> {
        if let Some(r) = self.page_refs.get(&id) {
            return Ok(Val::Ref(*r));
        }
        match self.kind(id)? {
            Kind::Missing => Ok(Val::Null),
            Kind::PageTree => self
                .pages_ref
                .map(Val::Ref)
                .ok_or_else(|| err_obj(ErrorCode::Internal, "page tree before its root", id)),
            Kind::Is(class) => self.object(id, class),
        }
    }

    /// What preserve mode makes of `id`. Only what changes how an object is written has a
    /// class of its own (content streams, fonts); every other object is `Raw`, so one object is
    /// one output object whichever key reached it.
    fn kind(&self, id: ObjectId) -> Result<Kind, PdfError> {
        let source = self.redirects.aliases.get(&id).copied().unwrap_or(id);
        let ident = ObjectIdentifier::new(source.num as i32, i32::from(source.generation));
        let Some(obj) = self.doc.pdf().xref().get::<Object<'a>>(ident) else {
            return Ok(Kind::Missing);
        };
        let stream = matches!(obj, Object::Stream(_));
        let val = self.to_val(&obj, 0)?;
        let Val::Dict(d) = &val else {
            return Ok(Kind::Is(C::Raw));
        };
        let name = |k: &str| match get(d, k) {
            Some(Val::Name(n)) => Some(n.as_slice()),
            _ => None,
        };
        Ok(Kind::Is(match (stream, name("Type"), name("Subtype")) {
            (true, _, Some(b"Form")) => C::Form,
            (true, _, _) if matches!(get(d, "PatternType"), Some(Val::Int(1))) => C::Tiling,
            (false, Some(b"Pages"), _) => return Ok(Kind::PageTree),
            (false, Some(b"Font"), _) if refine(C::Font, &val, id).is_ok() => C::Font,
            _ => C::Raw,
        }))
    }

    fn inline(&self, id: ObjectId) -> Result<Val, PdfError> {
        match self.load(id)? {
            (_, Some(_)) => Err(err_obj(
                ErrorCode::MalformedPdf,
                "stream in a plain value",
                id,
            )),
            (v, None) => Ok(v),
        }
    }

    fn emit(&mut self, v: &Val, spec: V, depth: usize) -> Result<Val, PdfError> {
        if depth > MAX_DEPTH {
            return Err(err(ErrorCode::MalformedPdf, "object nesting too deep"));
        }
        let d = depth + 1;
        let spec = if self.preserve && matches!(spec, V::Plain) {
            V::Any
        } else {
            spec
        };
        Ok(match (spec, v) {
            (_, Val::New(i)) => self.extra(*i)?,
            (V::Any, Val::Src(id)) => self.any(*id)?,
            (V::Any, Val::Arr(a)) => Val::Arr(self.emit_all(a, V::Any, d)?),
            (V::Any, Val::Dict(e)) => {
                let mut out = Vec::new();
                for (k, x) in e.iter().filter(|(_, x)| *x != Val::Null) {
                    out.push((k.clone(), self.emit(x, V::Any, d)?));
                }
                Val::Dict(out)
            }
            (V::Any, other) => other.clone(),
            (One(C::ColorSpace), _) => self.color_space(v, d)?,
            (V::Plain, Val::Src(id)) => {
                let inner = self.inline(*id)?;
                self.emit(&inner, Plain, d)?
            }
            (V::Plain, Val::Arr(a)) => Val::Arr(self.emit_all(a, Plain, d)?),
            (V::Plain, Val::Dict(e)) => {
                let mut out = Vec::new();
                for (k, x) in e.iter().filter(|(_, x)| *x != Val::Null) {
                    out.push((k.clone(), self.emit(x, Plain, d)?));
                }
                Val::Dict(out)
            }
            (One(c), Val::Src(id)) => self.object(*id, c)?,
            (One(c), Val::Dict(e)) => Val::Dict(self.emit_dict(e, c, d)?),
            (Many(c), Val::Src(id)) => match self.load(*id)? {
                (Val::Arr(items), None) => Val::Arr(self.emit_all(&items, One(c), d)?),
                (Val::Dict(_), _) | (_, Some(_)) => self.object(*id, c)?,
                (other, None) => self.emit(&other, Plain, d)?,
            },
            (Many(c), Val::Arr(a)) => Val::Arr(self.emit_all(a, One(c), d)?),
            (Many(c), _) => self.emit(v, One(c), d)?,
            (Map(c), Val::Src(id)) => {
                let inner = self.inline(*id)?;
                self.emit(&inner, Map(c), d)?
            }
            (Map(c), Val::Dict(e)) => {
                let mut out = Vec::new();
                for (k, x) in e {
                    let layer = c == C::PropList && self.layers_resolved && self.is_layer(x);
                    if *x == Val::Null || layer {
                        continue;
                    }
                    out.push((k.clone(), self.emit(x, One(c), d)?));
                }
                Val::Dict(out)
            }
            (V::Tuple(specs), Val::Src(id)) => {
                let inner = self.inline(*id)?;
                self.emit(&inner, V::Tuple(specs), d)?
            }
            (V::Tuple(specs), Val::Arr(a)) => {
                let mut out = Vec::new();
                for (i, x) in a.iter().enumerate() {
                    out.push(self.emit(x, specs.get(i).copied().unwrap_or(Plain), d)?);
                }
                Val::Arr(out)
            }
            (V::Plain, other) => other.clone(),
            (_, other) => self.emit(other, Plain, d)?,
        })
    }

    /// The output reference of added object `i`, written on first use with the references
    /// it holds resolved the same way.
    fn extra(&mut self, i: usize) -> Result<Val, PdfError> {
        if let Some(r) = self.extra_refs.get(&i) {
            return Ok(Val::Ref(*r));
        }
        let Some(extra) = self.extras.get(i) else {
            return Err(err(ErrorCode::Internal, "unknown added object"));
        };
        let r = self.alloc()?;
        self.extra_refs.insert(i, r);
        let mut val = self.resolve_new(&extra.val, 0)?;
        if let (Some(data), Val::Dict(e)) = (&extra.data, &mut val) {
            e.extend(flate());
            self.push(Node {
                id: r,
                val,
                data: Some(deflate(data)),
            })?;
        } else {
            self.push(Node {
                id: r,
                val,
                data: None,
            })?;
        }
        Ok(Val::Ref(r))
    }

    fn resolve_new(&mut self, v: &Val, depth: usize) -> Result<Val, PdfError> {
        if depth > MAX_DEPTH {
            return Err(err(ErrorCode::Internal, "added object nesting too deep"));
        }
        Ok(match v {
            Val::New(i) => self.extra(*i)?,
            Val::Arr(a) => Val::Arr(
                a.iter()
                    .map(|x| self.resolve_new(x, depth + 1))
                    .collect::<Result<_, _>>()?,
            ),
            Val::Dict(e) => Val::Dict(
                e.iter()
                    .map(|(k, x)| Ok((k.clone(), self.resolve_new(x, depth + 1)?)))
                    .collect::<Result<_, PdfError>>()?,
            ),
            other => other.clone(),
        })
    }

    fn emit_all(&mut self, items: &[Val], spec: V, depth: usize) -> Result<Vec<Val>, PdfError> {
        items.iter().map(|x| self.emit(x, spec, depth)).collect()
    }

    fn emit_dict(
        &mut self,
        entries: &Entries,
        class: Class,
        depth: usize,
    ) -> Result<Entries, PdfError> {
        let mut out = Vec::new();
        for (key, spec) in class.keys() {
            if let Some(v) = get(entries, key).filter(|v| **v != Val::Null) {
                out.push((key.as_bytes().to_vec(), self.emit(v, *spec, depth)?));
            }
        }
        if self.preserve {
            let known = |k: &[u8]| class.keys().iter().any(|(n, _)| n.as_bytes() == k);
            for (k, v) in entries.iter().filter(|(k, v)| *v != Val::Null && !known(k)) {
                out.push((k.clone(), self.emit(v, V::Any, depth)?));
            }
        }
        Ok(out)
    }

    /// Color space: a name, or an array whose parts are classified by the family name.
    fn color_space(&mut self, v: &Val, depth: usize) -> Result<Val, PdfError> {
        match v {
            Val::Src(id) => self.object(*id, C::ColorSpace),
            Val::Arr(a) => {
                let fam: &[V] = match a.first() {
                    Some(Val::Name(n)) => match n.as_slice() {
                        b"ICCBased" => &[Plain, One(C::IccStream)],
                        b"Indexed" | b"I" => &[Plain, One(C::ColorSpace), Plain, One(C::Data)],
                        b"Separation" | b"DeviceN" => {
                            &[Plain, Plain, One(C::ColorSpace), Many(C::Function)]
                        }
                        b"Pattern" => &[Plain, One(C::ColorSpace)],
                        _ => &[],
                    },
                    _ => &[],
                };
                // Separation/DeviceN: the attributes dictionary (5th item) is dropped.
                let keep = if !self.preserve
                    && matches!(a.first(), Some(Val::Name(n)) if n == b"Separation" || n == b"DeviceN")
                {
                    4
                } else {
                    a.len()
                };
                let mut out = Vec::new();
                for (i, x) in a.iter().take(keep).enumerate() {
                    out.push(self.emit(x, fam.get(i).copied().unwrap_or(Plain), depth)?);
                }
                Ok(Val::Arr(out))
            }
            other => self.emit(other, Plain, depth),
        }
    }

    // ---- objects ---------------------------------------------------------------------

    /// Emit every queued object (and the ones they reach) until none is left.
    pub fn drain(&mut self) -> Result<(), PdfError> {
        while let Some((id, class, r)) = self.queue.pop_front() {
            self.process(id, class, r)?;
        }
        Ok(())
    }

    fn process(&mut self, id: ObjectId, class: Class, r: Ref) -> Result<(), PdfError> {
        let repl = self.repl.get(&id);
        let source = self.redirects.aliases.get(&id).copied().unwrap_or(id);
        let (val, stream) = match repl {
            Some(n) => (
                Val::Dict(
                    n.dict
                        .iter()
                        .map(|(k, v)| (k.as_bytes().to_vec(), v.clone()))
                        .collect(),
                ),
                None,
            ),
            None => self.load(source)?,
        };
        let val = match val {
            // The stream's own framing is rewritten; a copied /Length or /Filter would lie.
            Val::Dict(e) if self.preserve && repl.is_none() && stream.is_some() => Val::Dict(
                e.into_iter()
                    .filter(|(k, _)| {
                        !matches!(
                            k.as_slice(),
                            b"Length"
                                | b"Filter"
                                | b"DecodeParms"
                                | b"DL"
                                | b"F"
                                | b"FFilter"
                                | b"FDecodeParms"
                        )
                    })
                    .collect(),
            ),
            other => other,
        };
        let val = self.edit_form_resources(id, val)?;
        let class = refine(class, &val, id)?;
        // A rebuilt ToUnicode replaces the source one, which must not be copied at all: an
        // orphaned copy would still hold the mapping of every removed code.
        let table = self.to_unicode;
        let rebuilt = matches!(class, C::SimpleFont | C::Type0Font | C::Type3Font)
            .then(|| table.get(&id))
            .flatten();
        let dict = match &val {
            Val::Dict(e) if rebuilt.is_some() => {
                let kept: Vec<_> = e
                    .iter()
                    .filter(|(k, _)| k != b"ToUnicode")
                    .cloned()
                    .collect();
                Val::Dict(self.emit_dict(&kept, class, 0)?)
            }
            Val::Dict(e) => Val::Dict(self.emit_dict(e, class, 0)?),
            other if class == C::ColorSpace => self.color_space(other, 0)?,
            other => self.emit(other, Plain, 0)?,
        };
        let (data, filter) = match (repl, &stream) {
            (Some(n), _) => (Some(deflate(&n.data)), flate()),
            (None, Some(s)) => {
                let (d, f) = self.stream_data(s, id, class)?;
                (Some(self.encoded.get(&id).cloned().unwrap_or(d)), f)
            }
            (None, None) => (None, Vec::new()),
        };
        let mut dict = dict;
        if let Val::Dict(e) = &mut dict {
            e.extend(filter);
            if let Some(cmap) = rebuilt {
                e.push((b"ToUnicode".to_vec(), self.plain_stream(cmap)?));
            }
        }
        self.push(Node {
            id: r,
            val: dict,
            data,
        })
    }

    /// A Form's dictionary with the XObject resources `Writer::add_form_xobject` and
    /// `drop_form_xobject` changed (copied, never edited in place).
    fn edit_form_resources(&self, id: ObjectId, val: Val) -> Result<Val, PdfError> {
        let xobjects = self.redirects.form_xobjects.get(&id);
        let pruned = self.redirects.pruned.get(&Owner::Object(id));
        if xobjects.is_none() && pruned.is_none() {
            return Ok(val);
        }
        let Val::Dict(mut entries) = val else {
            return Err(err_obj(ErrorCode::MalformedPdf, "Form is not a stream", id));
        };
        let Some(i) = entries.iter().position(|(k, _)| k == b"Resources") else {
            return Err(err_obj(
                ErrorCode::InvalidArgument,
                "Form has no /Resources",
                id,
            ));
        };
        let mut res = entries[i].1.clone();
        if let Some((add, drop)) = xobjects {
            res = self.with_entries(Some(res), b"XObject", add, drop)?;
        }
        if let Some(p) = pruned {
            res = self.prune(res, p)?;
        }
        entries[i].1 = res;
        Ok(Val::Dict(entries))
    }

    /// `res` without the (category, name) entries of `pruned`.
    fn prune(&self, mut res: Val, pruned: &[(&'static str, Vec<u8>)]) -> Result<Val, PdfError> {
        let mut seen: Vec<&str> = Vec::new();
        for (category, _) in pruned {
            if !seen.contains(category) {
                seen.push(category);
                let names: Vec<Vec<u8>> = pruned
                    .iter()
                    .filter(|(c, _)| c == category)
                    .map(|(_, n)| n.clone())
                    .collect();
                res = self.with_entries(Some(res), category.as_bytes(), &[], &names)?;
            }
        }
        Ok(res)
    }

    /// Encoded bytes and `/Filter`, `/DecodeParms` entries for a source stream. Content
    /// streams are re-emitted from tokens and flate-compressed. Other streams keep their
    /// bytes when the whole filter chain is allowlisted (Flate, DCT, JPX, JBIG2, CCITT) and
    /// are otherwise decoded (strict, capped) and flate-compressed (LZW, RunLength, ASCII).
    fn stream_data(
        &mut self,
        s: &Stream<'a>,
        id: ObjectId,
        class: Class,
    ) -> Result<(Vec<u8>, Entries), PdfError> {
        if class.is_content() {
            let cs = reader::decode_content(s, self.limits, &mut self.budget)?;
            let subs = self.spans.get(&id).map_or(&[][..], Vec::as_slice);
            return Ok((deflate(&content::reemit(&cs.data, id, subs)?), flate()));
        }
        if !class.allows_data_stream() {
            return Err(err_obj(
                ErrorCode::MalformedPdf,
                "stream where a dictionary belongs",
                id,
            ));
        }
        let dict = s.dict();
        let declared = if dict.get::<Name<'_>>(FILTER).is_some() {
            1
        } else {
            dict.get::<Array<'_>>(FILTER)
                .map_or(0, |a| a.raw_iter().count())
        };
        let fs = s.filters();
        if fs.len() != declared {
            return Err(err_obj(ErrorCode::DecodeFailed, "unsupported filter", id));
        }
        let image = |f: &Filter| {
            matches!(
                f,
                Filter::DctDecode
                    | Filter::JpxDecode
                    | Filter::Jbig2Decode
                    | Filter::CcittFaxDecode
            )
        };
        if !fs.is_empty() && fs.iter().all(|f| image(f) || *f == Filter::FlateDecode) {
            let raw = s
                .raw_data_strict()
                .map_err(|_| err_obj(ErrorCode::DecodeFailed, "decryption failed", id))?;
            self.budget.charge_bytes(raw.len())?;
            // Refuse before the copy below allocates.
            limits::check_output((self.bytes + raw.len()) as u64, !self.preserve, self.limits)?;
            let names: Vec<Val> = fs
                .iter()
                .map(|f| {
                    name_val(match f {
                        Filter::DctDecode => b"DCTDecode".as_slice(),
                        Filter::JpxDecode => b"JPXDecode",
                        Filter::Jbig2Decode => b"JBIG2Decode",
                        Filter::CcittFaxDecode => b"CCITTFaxDecode",
                        _ => b"FlateDecode",
                    })
                })
                .collect();
            let filter = if names.len() == 1 {
                names[0].clone()
            } else {
                Val::Arr(names)
            };
            let mut out = vec![(b"Filter".to_vec(), filter)];
            if let Some(m) = dict.get_raw::<Object<'a>>(b"DecodeParms") {
                let v = self.maybe(m, 0)?;
                out.push((
                    b"DecodeParms".to_vec(),
                    self.emit(&v, Many(C::DecodeParms), 0)?,
                ));
            }
            return Ok((raw.into_owned(), out));
        }
        if fs.iter().any(image) {
            return Err(err_obj(
                ErrorCode::DecodeFailed,
                "image filter behind a re-encoded filter",
                id,
            ));
        }
        let data = limits::decode_stream(s, id, self.limits, &mut self.budget)?;
        Ok((deflate(&data), flate()))
    }

    fn resolve(&self, v: &Val) -> Result<Val, PdfError> {
        match v {
            Val::Src(id) => self.inline(*id),
            other => Ok(other.clone()),
        }
    }

    /// An optional-content group or membership dictionary (`/Properties` entry).
    fn is_layer(&self, v: &Val) -> bool {
        let d = match v {
            Val::Src(id) => self.load(*id).ok().map(|(v, _)| v),
            other => Some(other.clone()),
        };
        matches!(d, Some(Val::Dict(e)) if matches!(get(&e, "Type"), Some(Val::Name(n)) if n == b"OCG" || n == b"OCMD"))
    }

    /// Hidden layers would turn visible once `/OCProperties` is dropped (flattening is a later
    /// task, spec 8.2): refuse instead.
    fn refuse_hidden_layers(&self) -> Result<(), PdfError> {
        let root = object_id(self.doc.pdf().xref().root_id().into());
        let (Val::Dict(cat), _) = self.load(root)? else {
            return Err(err_obj(
                ErrorCode::MalformedPdf,
                "catalog is not a dictionary",
                root,
            ));
        };
        let Some(Val::Dict(oc)) = get(&cat, "OCProperties")
            .map(|v| self.resolve(v))
            .transpose()?
        else {
            return Ok(());
        };
        let Some(Val::Dict(d)) = get(&oc, "D").map(|v| self.resolve(v)).transpose()? else {
            return Ok(());
        };
        let off = matches!(get(&d, "OFF").map(|v| self.resolve(v)).transpose()?, Some(Val::Arr(a)) if !a.is_empty());
        let base_off = matches!(get(&d, "BaseState"), Some(Val::Name(n)) if n == b"OFF");
        if off || base_off {
            return Err(err_obj(
                ErrorCode::OptionalContentUnsupported,
                "hidden optional content",
                root,
            ));
        }
        Ok(())
    }

    fn inherited(&self, page: &Dict<'a>, key: &[u8]) -> Result<Option<Val>, PdfError> {
        let mut d = page.clone();
        for _ in 0..MAX_DEPTH {
            if let Some(m) = d.get_raw::<Object<'a>>(key) {
                return Ok(Some(self.maybe(m, 0)?));
            }
            match d.get::<Dict<'a>>(PARENT) {
                Some(p) => d = p,
                None => return Ok(None),
            }
        }
        Err(err(ErrorCode::MalformedPdf, "page tree too deep"))
    }

    /// Catalog, page tree root and one object per page (ids 1, 2, 3..), with inherited
    /// attributes resolved onto each page. Everything the pages reach is emitted too.
    pub fn build(
        &mut self,
        appended: &BTreeMap<usize, Vec<u8>>,
        [page_fonts, page_xobjects]: [&PageFonts; 2],
        dropped_xobjects: &BTreeMap<usize, Vec<Vec<u8>>>,
    ) -> Result<Vec<Ref>, PdfError> {
        self.alloc()?;
        let pages = self.alloc()?;
        if !self.layers_resolved && !self.preserve {
            self.refuse_hidden_layers()?;
        }
        let refs = (0..self.doc.page_count())
            .map(|_| self.alloc())
            .collect::<Result<Vec<_>, _>>()?;
        if self.preserve {
            self.pages_ref = Some(pages);
            for (i, r) in refs.iter().enumerate() {
                if let Some(id) = self.doc.page(i)?.raw().obj_id() {
                    self.page_refs.insert(object_id(id.into()), *r);
                }
            }
        }
        let mut q: Option<Val> = None;
        for (i, &r) in refs.iter().enumerate() {
            let raw = self.doc.page(i)?.raw().clone();
            let mut e: Entries = vec![
                (b"Type".to_vec(), name_val(b"Page")),
                (b"Parent".to_vec(), Val::Ref(pages)),
            ];
            for (key, spec) in [
                ("MediaBox", Plain),
                ("CropBox", Plain),
                ("Rotate", Plain),
                ("Resources", One(C::Resources)),
            ] {
                let mut v = self
                    .inherited(&raw, key.as_bytes())?
                    .filter(|v| *v != Val::Null);
                if key == "Resources" {
                    let none = Vec::new();
                    let dropped = dropped_xobjects.get(&i).unwrap_or(&none);
                    for (category, added, drop) in [
                        (&b"Font"[..], page_fonts, &none),
                        (b"XObject", page_xobjects, dropped),
                    ] {
                        if added.contains_key(&i) || !drop.is_empty() {
                            let add = added.get(&i).map_or(&[][..], Vec::as_slice);
                            v = Some(self.with_entries(v, category, add, drop)?);
                        }
                    }
                    if let Some(p) = self.redirects.pruned.get(&Owner::Page(i))
                        && let Some(res) = v.take()
                    {
                        v = Some(self.prune(res, p)?);
                    }
                }
                if let Some(v) = v {
                    e.push((key.as_bytes().to_vec(), self.emit(&v, spec, 0)?));
                }
            }
            for (key, spec) in [("UserUnit", Plain), ("Group", One(C::Group))] {
                if let Some(m) = raw.get_raw::<Object<'a>>(key) {
                    let v = self.maybe(m, 0)?;
                    if v != Val::Null {
                        e.push((key.as_bytes().to_vec(), self.emit(&v, spec, 0)?));
                    }
                }
            }
            let annots = self.link_annots(i, &refs)?;
            if !annots.is_empty() {
                e.push((b"Annots".to_vec(), Val::Arr(annots)));
            }
            let mut parts = Vec::new();
            if self.preserve {
                for (k, m) in raw.entries() {
                    let k = k.deref();
                    let kept = matches!(
                        k,
                        b"Type"
                            | b"Parent"
                            | b"MediaBox"
                            | b"CropBox"
                            | b"Rotate"
                            | b"Resources"
                            | b"UserUnit"
                            | b"Group"
                            | b"Contents"
                    );
                    let v = self.maybe(m, 0)?;
                    if !kept && v != Val::Null {
                        e.push((k.to_vec(), self.emit(&v, V::Any, 0)?));
                    }
                }
            }
            if let Some(m) = raw.get_raw::<Object<'a>>(b"Contents") {
                for id in self.content_ids(m)? {
                    let id = self
                        .redirects
                        .page_streams
                        .get(&(i, id))
                        .copied()
                        .unwrap_or(id);
                    parts.push(self.object(id, C::Content)?);
                }
            }
            if let Some(ops) = appended.get(&i) {
                if parts.is_empty() {
                    parts.push(self.plain_stream(ops)?);
                } else {
                    if q.is_none() {
                        q = Some(self.plain_stream(b"q\n")?);
                    }
                    parts.insert(0, q.clone().expect("set above"));
                    let tail = [b"Q\n".as_slice(), ops].concat();
                    parts.push(self.plain_stream(&tail)?);
                }
            }
            if !parts.is_empty() {
                e.push((b"Contents".to_vec(), Val::Arr(parts)));
            }
            self.push(Node {
                id: r,
                val: Val::Dict(e),
                data: None,
            })?;
        }
        if self.preserve {
            self.preserve_roots(pages, &refs)?;
        }
        self.drain()?;
        self.nodes.sort_by_key(|n| n.id.get());
        let mut refs = refs;
        if !self.preserve {
            super::shrink::shrink(&mut self.nodes, &mut refs, self.dedupe)?;
        }
        Ok(refs)
    }

    /// One fresh annotation object per kept link of `page`, from the allowlist of ADR 0014:
    /// nothing of the input annotation is copied. A destination names its page by output ref.
    fn link_annots(&mut self, page: usize, pages: &[Ref]) -> Result<Vec<Val>, PdfError> {
        let num = |v: f64| Val::Real(v as f32);
        let mut out = Vec::new();
        let links = self.links;
        for l in links.iter().filter(|l| l.page == page) {
            let mut e: Entries = vec![
                (b"Type".to_vec(), name_val(b"Annot")),
                (b"Subtype".to_vec(), name_val(b"Link")),
                (
                    b"Rect".to_vec(),
                    Val::Arr(l.rect.iter().map(|v| num(*v)).collect()),
                ),
            ];
            if !l.quad_points.is_empty() {
                let q = l.quad_points.iter().map(|v| num(*v)).collect();
                e.push((b"QuadPoints".to_vec(), Val::Arr(q)));
            }
            let zero = Val::Int(0);
            e.push((b"Border".to_vec(), Val::Arr(vec![zero; 3])));
            if let Some(h) = l.highlight {
                e.push((b"H".to_vec(), name_val(&[h])));
            }
            match &l.target {
                LinkTarget::Uri(uri) => e.push((
                    b"A".to_vec(),
                    Val::Dict(vec![
                        (b"S".to_vec(), name_val(b"URI")),
                        (b"URI".to_vec(), Val::Str(uri.clone())),
                    ]),
                )),
                LinkTarget::Dest { page, view, args } => {
                    let target = pages
                        .get(*page)
                        .ok_or_else(|| err(ErrorCode::Internal, "link destination page"))?;
                    let mut d = vec![Val::Ref(*target), name_val(view.as_bytes())];
                    d.extend(args.iter().map(|a| a.map_or(Val::Null, num)));
                    e.push((b"Dest".to_vec(), Val::Arr(d)));
                }
            }
            let id = self.alloc()?;
            self.push(Node {
                id,
                val: Val::Dict(e),
                data: None,
            })?;
            out.push(Val::Ref(id));
        }
        Ok(out)
    }

    /// Preserve mode: the catalog with every entry of the input's (the page tree is ours), the
    /// page tree root, and the trailer's `/Info`.
    fn preserve_roots(&mut self, pages: Ref, refs: &[Ref]) -> Result<(), PdfError> {
        let xref = self.doc.pdf().xref();
        let root = object_id(xref.root_id().into());
        let (Val::Dict(cat), _) = self.load(root)? else {
            return Err(err_obj(
                ErrorCode::MalformedPdf,
                "catalog is not a dictionary",
                root,
            ));
        };
        let mut e: Entries = vec![
            (b"Type".to_vec(), name_val(b"Catalog")),
            (b"Pages".to_vec(), Val::Ref(pages)),
        ];
        for (k, v) in cat
            .iter()
            .filter(|(k, v)| *v != Val::Null && k != b"Type" && k != b"Pages")
        {
            e.push((k.clone(), self.emit(v, V::Any, 0)?));
        }
        let catalog = Val::Dict(e);
        let kids = Val::Arr(refs.iter().map(|r| Val::Ref(*r)).collect());
        let tree = Val::Dict(vec![
            (b"Type".to_vec(), name_val(b"Pages")),
            (b"Kids".to_vec(), kids),
            (b"Count".to_vec(), Val::Int(refs.len() as i32)),
        ]);
        for (id, val) in [(Ref::new(1), catalog), (pages, tree)] {
            self.push(Node {
                id,
                val,
                data: None,
            })?;
        }
        if let Some(info) = xref.info_id()
            && let Val::Ref(r) = self.any(object_id(info.into()))?
        {
            self.info = Some(r);
        }
        Ok(())
    }

    /// `/Resources` with `added` put into its `category` dictionary (`/Font`, `/XObject`),
    /// copied, never edited in place.
    fn with_entries(
        &self,
        res: Option<Val>,
        category: &[u8],
        added: &[(Vec<u8>, Val)],
        drop: &[Vec<u8>],
    ) -> Result<Val, PdfError> {
        let bad = || err(ErrorCode::MalformedPdf, "resources are not a dictionary");
        let dict = |v: Option<Val>| match v.map(|v| self.resolve(&v)).transpose()? {
            None | Some(Val::Null) => Ok(Vec::new()),
            Some(Val::Dict(e)) => Ok(e),
            Some(_) => Err(bad()),
        };
        let mut res = dict(res)?;
        let mut map = dict(
            res.iter()
                .find(|(k, _)| k == category)
                .map(|(_, v)| v.clone()),
        )?;
        map.retain(|(k, _)| !drop.contains(k));
        for (name, val) in added {
            if map.iter().any(|(k, _)| k == name) {
                return Err(err(ErrorCode::InvalidArgument, "resource name in use"));
            }
            map.push((name.clone(), val.clone()));
        }
        res.retain(|(k, _)| k != category);
        if !map.is_empty() {
            res.push((category.to_vec(), Val::Dict(map)));
        }
        Ok(Val::Dict(res))
    }

    fn content_ids(&self, m: MaybeRef<Object<'a>>) -> Result<Vec<ObjectId>, PdfError> {
        let bad = || {
            err(
                ErrorCode::MalformedPdf,
                "/Contents is not a stream or an array of streams",
            )
        };
        let srcs = |items: Vec<Val>| {
            items
                .into_iter()
                .map(|v| {
                    if let Val::Src(id) = v {
                        Ok(id)
                    } else {
                        Err(bad())
                    }
                })
                .collect::<Result<Vec<_>, _>>()
        };
        match self.maybe(m, 0)? {
            Val::Src(id) => match self.load(id)? {
                (_, Some(_)) => Ok(vec![id]),
                (Val::Arr(items), None) => srcs(items),
                _ => Err(bad()),
            },
            Val::Arr(items) => srcs(items),
            Val::Null => Ok(vec![]),
            _ => Err(bad()),
        }
    }
}

fn flate() -> Entries {
    vec![(b"Filter".to_vec(), name_val(b"FlateDecode"))]
}
