//! Strict, fallible reader (spec §0.1): every failure is a typed error with the object it
//! happened in. Nothing is silently skipped, emptied or repaired.
use crate::limits::{self, Budget};
use foliark_model::{ErrorCode, ErrorDetails, FoliarkError, Limits, ObjectId};
use hayro::hayro_syntax::content::TypedIter;
use hayro::hayro_syntax::object::dict::keys::{CONTENTS, FILTER};
use hayro::hayro_syntax::object::{Array, Name, ObjRef, Object, Stream};
use hayro::hayro_syntax::page::Page;
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};
use std::fmt;

mod preflight;
mod provenance;
pub use provenance::{Item, ItemKind};

/// Why a read failed. [`ReaderError::code`] is the spec §12 error code and
/// `FoliarkError::from` adds the details (object, limit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderError {
    /// Unparsable, no `%PDF-` header in the first 1024 bytes.
    MalformedPdf,
    /// The xref table was rebuilt by scanning the file and `allow_repaired` is off.
    RepairedPdf,
    PasswordRequired,
    PasswordIncorrect,
    EncryptionUnsupported,
    PageOutOfRange {
        page: usize,
        count: usize,
    },
    /// A filter or decryption failed, or the Flate data is corrupt or truncated.
    DecodeFailed {
        obj: ObjectId,
    },
    /// The stream names a filter this reader does not implement.
    UnsupportedFilter {
        obj: ObjectId,
    },
    /// `/Contents` points at an object that cannot be read (for example a truncated file).
    ContentUnreadable {
        obj: ObjectId,
    },
    /// The decoded content stream stops parsing before its end.
    ContentSyntax {
        obj: ObjectId,
        offset: usize,
    },
    /// A Form draws itself, directly or through other Forms.
    FormCycle {
        obj: ObjectId,
    },
    /// A spec §9 limit was exceeded. `limit` is the `Limits` field name; `actual` is a lower
    /// bound when the check stops at the first byte over the cap.
    LimitExceeded {
        limit: &'static str,
        actual: u64,
        max: u64,
    },
}

impl ReaderError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::MalformedPdf
            | Self::ContentUnreadable { .. }
            | Self::ContentSyntax { .. }
            | Self::FormCycle { .. } => ErrorCode::MalformedPdf,
            Self::PageOutOfRange { .. } => ErrorCode::InvalidArgument,
            Self::RepairedPdf => ErrorCode::RepairedPdf,
            Self::PasswordRequired => ErrorCode::PasswordRequired,
            Self::PasswordIncorrect => ErrorCode::PasswordIncorrect,
            Self::EncryptionUnsupported => ErrorCode::EncryptionUnsupported,
            Self::DecodeFailed { .. } | Self::UnsupportedFilter { .. } => ErrorCode::DecodeFailed,
            Self::LimitExceeded { .. } => ErrorCode::LimitExceeded,
        }
    }

    fn details(&self) -> Option<ErrorDetails> {
        let obj_ref = |o: &ObjectId| Some(format!("{} {} R", o.num, o.generation));
        match self {
            Self::DecodeFailed { obj }
            | Self::UnsupportedFilter { obj }
            | Self::ContentUnreadable { obj }
            | Self::ContentSyntax { obj, .. }
            | Self::FormCycle { obj } => Some(ErrorDetails {
                obj_ref: obj_ref(obj),
                ..Default::default()
            }),
            Self::LimitExceeded { limit, actual, max } => Some(ErrorDetails {
                limit: Some((*limit).into()),
                actual: Some(*actual as f64),
                max: Some(*max as f64),
                ..Default::default()
            }),
            _ => None,
        }
    }
}

impl fmt::Display for ReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {self:?}", self.code())
    }
}

impl std::error::Error for ReaderError {}

impl From<&ReaderError> for FoliarkError {
    fn from(e: &ReaderError) -> Self {
        FoliarkError {
            code: e.code(),
            message: e.to_string(),
            details: e.details(),
        }
    }
}

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
}

impl Document {
    /// Open with the default [`Limits`].
    pub fn open(
        bytes: Vec<u8>,
        password: Option<&str>,
        allow_repaired: bool,
    ) -> Result<Self, ReaderError> {
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
    ) -> Result<Self, ReaderError> {
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
            return Err(ReaderError::MalformedPdf);
        }
        let pdf = Pdf::new_with_password(bytes, password).map_err(|e| match e {
            LoadPdfError::Decryption(DecryptionError::PasswordProtected) if password.is_empty() => {
                ReaderError::PasswordRequired
            }
            LoadPdfError::Decryption(DecryptionError::PasswordProtected) => {
                ReaderError::PasswordIncorrect
            }
            LoadPdfError::Decryption(DecryptionError::UnsupportedAlgorithm) => {
                ReaderError::EncryptionUnsupported
            }
            _ => ReaderError::MalformedPdf,
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

    pub(crate) fn page(&self, index: usize) -> Result<&Page<'_>, ReaderError> {
        let pages = self.pdf.pages();
        pages.get(index).ok_or(ReaderError::PageOutOfRange {
            page: index,
            count: pages.len(),
        })
    }

    pub(crate) fn check_repaired(&self) -> Result<(), ReaderError> {
        if self.was_repaired() && !self.allow_repaired {
            return Err(ReaderError::RepairedPdf);
        }
        Ok(())
    }

    /// The decoded content streams of a page, in drawing order. A page without `/Contents` is
    /// `Ok(vec![])`; an empty stream is `Ok` with empty data. Everything else that makes the
    /// content unreadable is an `Err`, never an empty page.
    pub fn page_contents(&self, index: usize) -> Result<Vec<ContentStream>, ReaderError> {
        self.page_contents_with(index, &mut Budget::new(&self.limits))
    }

    pub(crate) fn page_contents_with(
        &self,
        index: usize,
        budget: &mut Budget,
    ) -> Result<Vec<ContentStream>, ReaderError> {
        let dict = self.page(index)?.raw();
        let unreadable = |r: Option<ObjRef>| ReaderError::ContentUnreadable {
            obj: r.map(object_id).unwrap_or(ObjectId {
                num: 0,
                generation: 0,
            }),
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
) -> Result<ContentStream, ReaderError> {
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
        return Err(ReaderError::UnsupportedFilter { obj });
    }
    let data = limits::decode_stream(stream, obj, limits, budget)?.into_owned();
    // shortcut: an invalid path operator that is the very last instruction goes unnoticed.
    let mut ops = TypedIter::new(&data);
    while ops.next().is_some() {}
    if ops.offset() < data.len() {
        return Err(ReaderError::ContentSyntax {
            obj,
            offset: ops.offset(),
        });
    }
    Ok(ContentStream { obj, data })
}
