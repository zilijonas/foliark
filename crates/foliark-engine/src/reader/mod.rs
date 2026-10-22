//! Strict, fallible reader (spec §0.1): every failure is a typed error with the object it
//! happened in. Nothing is silently skipped, emptied or repaired.
use crate::limits::{self, Budget};
use foliark_model::{ErrorCode, Limits, ObjectId, PageGeometry, PdfError, page_geometry};
use hayro::hayro_syntax::content::TypedIter;
use hayro::hayro_syntax::object::dict::keys::{CONTENTS, FILTER};
use hayro::hayro_syntax::object::{Array, Dict, Name, ObjRef, Object, Stream};
use hayro::hayro_syntax::page::Page;
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};
use std::cell::OnceCell;

mod preflight;

pub(crate) fn object_id(r: ObjRef) -> ObjectId {
    ObjectId {
        num: r.obj_number.max(0) as u32,
        generation: r.gen_number.clamp(0, i32::from(u16::MAX)) as u16,
    }
}

/// One decoded content stream of a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentStream {
    pub obj: ObjectId,
    pub data: Vec<u8>,
}

pub struct Document {
    pdf: Pdf,
    allow_repaired: bool,
    limits: Limits,
    /// Result of the whole-document budget scan, filled on first use.
    scanned: OnceCell<Result<(), PdfError>>,
}

impl Document {
    /// Open with the default [`Limits`].
    pub fn open(
        bytes: Vec<u8>,
        password: Option<&str>,
        allow_repaired: bool,
    ) -> Result<Self, PdfError> {
        Self::open_with_limits(bytes, password, allow_repaired, Limits::default())
    }

    /// Open a file. Nothing is mutated and no output is produced. Input size is checked before
    /// parsing, object and page counts (the actual ones) right after. A missing password is
    /// `PasswordRequired`, a wrong one `PasswordIncorrect`; the user and the owner password
    /// both open the file.
    pub fn open_with_limits(
        bytes: Vec<u8>,
        password: Option<&str>,
        allow_repaired: bool,
        limits: Limits,
    ) -> Result<Self, PdfError> {
        limits::check(
            "maxInputBytes",
            bytes.len() as u64,
            limits.max_input_bytes.into(),
        )?;
        let password = password.unwrap_or("");
        if !bytes[..bytes.len().min(1024)]
            .windows(5)
            .any(|w| w == b"%PDF-")
        {
            return Err(PdfError::new(ErrorCode::MalformedPdf));
        }
        let pdf = Pdf::new_with_password(bytes, password).map_err(|e| match e {
            LoadPdfError::Decryption(DecryptionError::PasswordProtected) if password.is_empty() => {
                PdfError::new(ErrorCode::PasswordRequired)
            }
            LoadPdfError::Decryption(DecryptionError::PasswordProtected) => {
                PdfError::new(ErrorCode::PasswordIncorrect)
            }
            LoadPdfError::Decryption(DecryptionError::UnsupportedAlgorithm) => {
                PdfError::new(ErrorCode::EncryptionUnsupported)
            }
            _ => PdfError::new(ErrorCode::MalformedPdf),
        })?;
        limits::check("maxObjects", pdf.len() as u64, limits.max_objects.into())?;
        limits::check(
            "maxPages",
            pdf.pages().len() as u64,
            limits.max_pages.into(),
        )?;
        let doc = Self {
            pdf,
            allow_repaired,
            limits,
            scanned: OnceCell::new(),
        };
        doc.check_repaired()?;
        Ok(doc)
    }

    /// Whether the xref table was rebuilt. Repair is lazy, so this can turn true after `open`
    /// (and then fails the next read unless `allow_repaired`).
    pub fn was_repaired(&self) -> bool {
        self.pdf.was_repaired()
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn pdf(&self) -> &Pdf {
        &self.pdf
    }

    pub fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    pub(crate) fn page(&self, index: usize) -> Result<&Page<'_>, PdfError> {
        let pages = self.pdf.pages();
        pages.get(index).ok_or(PdfError::page_out_of_range(index))
    }

    /// The spec §2 geometry of a page: boxes, inherited /Rotate, /UserUnit and both transforms.
    pub fn page_geometry(&self, index: usize) -> Result<PageGeometry, PdfError> {
        let page = self.page(index)?;
        let bounds = |r: hayro::hayro_syntax::object::Rect| [r.x0, r.y0, r.x1, r.y1];
        let malformed = || PdfError::new(ErrorCode::MalformedPdf);
        // /Rotate is inheritable, a damaged /Parent chain may loop (bounded), and anything but
        // an integer is malformed.
        let mut node = page.raw().clone();
        let mut rotate = None;
        for _ in 0..64 {
            if node.contains_key("Rotate") {
                let v = node.get::<f64>("Rotate").ok_or_else(malformed)?;
                if v.fract() != 0.0 || v.abs() > 1e9 {
                    return Err(malformed());
                }
                rotate = Some(v as i64);
                break;
            }
            match node.get::<Dict<'_>>("Parent") {
                Some(parent) => node = parent,
                None => break,
            }
        }
        let unit = match page.raw().get::<f64>("UserUnit") {
            Some(u) => u,
            None if page.raw().contains_key("UserUnit") => return Err(malformed()),
            None => 1.0,
        };
        page_geometry(
            index as u32,
            bounds(page.media_box()),
            Some(bounds(page.crop_box())),
            rotate.unwrap_or(0),
            unit,
        )
    }

    pub(crate) fn check_repaired(&self) -> Result<(), PdfError> {
        if self.was_repaired() && !self.allow_repaired {
            return Err(PdfError::new(ErrorCode::RepairedPdf));
        }
        Ok(())
    }

    /// The decoded content streams of a page, in drawing order. A page without `/Contents` is
    /// `Ok(vec![])`; an empty stream is `Ok` with empty data. Everything else that makes the
    /// content unreadable is an `Err`, never an empty page.
    pub fn page_contents(&self, index: usize) -> Result<Vec<ContentStream>, PdfError> {
        self.page_contents_with(index, &mut Budget::new(&self.limits))
    }

    pub(crate) fn page_contents_with(
        &self,
        index: usize,
        budget: &mut Budget,
    ) -> Result<Vec<ContentStream>, PdfError> {
        let dict = self.page(index)?.raw();
        let unreadable = |r: Option<ObjRef>| {
            let none = ObjectId {
                num: 0,
                generation: 0,
            };
            PdfError::at(ErrorCode::MalformedPdf, r.map(object_id).unwrap_or(none))
        };
        let xref = self.pdf.xref();
        let streams: Vec<Stream<'_>> = match dict.get::<Object<'_>>(CONTENTS) {
            Some(Object::Stream(s)) => vec![s],
            Some(Object::Array(a)) => contents_array(&a, xref).map_err(unreadable)?,
            Some(Object::Null(_)) | None => match dict.get_ref(CONTENTS) {
                // A reference that resolves to nothing: the object is damaged or cut off.
                Some(r) => return Err(unreadable(Some(r))),
                None => vec![],
            },
            Some(_) => return Err(unreadable(dict.get_ref(CONTENTS))),
        };
        let out = streams
            .iter()
            .map(|s| decode_content(s, &self.limits, budget))
            .collect::<Result<_, _>>()?;
        self.check_repaired()?;
        Ok(out)
    }
}

fn contents_array<'a>(
    array: &Array<'a>,
    xref: &'a hayro::hayro_syntax::xref::XRef,
) -> Result<Vec<Stream<'a>>, Option<ObjRef>> {
    array
        .raw_iter()
        .map(|item| {
            let r = item.as_obj_ref();
            xref.get::<Stream<'_>>(r.ok_or(None)?.into()).ok_or(r)
        })
        .collect()
}

fn decode_content(
    stream: &Stream<'_>,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<ContentStream, PdfError> {
    let obj = object_id(stream.obj_id().into());
    let dict = stream.dict();
    let named = dict
        .get::<Name<'_>>(FILTER)
        .map(|_| 1)
        .or_else(|| {
            dict.get::<Array<'_>>(FILTER)
                .map(|a| a.iter::<Name<'_>>().count())
        })
        .unwrap_or(0);
    // The parser drops filter names it does not know instead of failing.
    if named != stream.filters().len() {
        return Err(PdfError::at(ErrorCode::DecodeFailed, obj));
    }
    let data = limits::decode_stream(stream, obj, limits, budget)?.into_owned();
    // shortcut: an invalid path operator that is the very last instruction goes unnoticed.
    let mut ops = TypedIter::new(&data);
    while ops.next().is_some() {}
    if ops.offset() < data.len() {
        return Err(PdfError::at(ErrorCode::MalformedPdf, obj));
    }
    Ok(ContentStream { obj, data })
}
