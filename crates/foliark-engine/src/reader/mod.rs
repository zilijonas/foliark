//! Strict, fallible reader (spec §0.1): every failure is a typed error with the object it
//! happened in. Nothing is silently skipped, emptied or repaired.
use foliark_model::ObjectId;
use hayro::hayro_syntax::content::TypedIter;
use hayro::hayro_syntax::object::dict::keys::{CONTENTS, FILTER};
use hayro::hayro_syntax::object::{Array, Name, ObjRef, Object, Stream};
use hayro::hayro_syntax::page::Page;
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};
use std::fmt;

mod provenance;
pub use provenance::{Item, ItemKind, MAX_NESTING};

/// Why a read failed. [`ReaderError::code`] is the spec §12 error code; `foliark-model` gets the
/// shared `PdfError` with the `ErrorCode` enum in a later task.
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
    /// Forms nest deeper than [`MAX_NESTING`].
    NestingLimit {
        limit: u32,
    },
}

impl ReaderError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::MalformedPdf
            | Self::ContentUnreadable { .. }
            | Self::ContentSyntax { .. }
            | Self::FormCycle { .. }
            | Self::PageOutOfRange { .. } => "MALFORMED_PDF",
            Self::RepairedPdf => "REPAIRED_PDF",
            Self::PasswordRequired => "PASSWORD_REQUIRED",
            Self::PasswordIncorrect => "PASSWORD_INCORRECT",
            Self::EncryptionUnsupported => "ENCRYPTION_UNSUPPORTED",
            Self::DecodeFailed { .. } | Self::UnsupportedFilter { .. } => "DECODE_FAILED",
            Self::NestingLimit { .. } => "LIMIT_EXCEEDED",
        }
    }
}

impl fmt::Display for ReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {self:?}", self.code())
    }
}

impl std::error::Error for ReaderError {}

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
}

impl Document {
    pub fn open(
        bytes: Vec<u8>,
        password: Option<&str>,
        allow_repaired: bool,
    ) -> Result<Self, ReaderError> {
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
        let doc = Self {
            pdf,
            allow_repaired,
        };
        doc.check_repaired()?;
        Ok(doc)
    }

    /// Whether the xref table was rebuilt. Repair is lazy, so this can turn true after `open`
    /// (and then fails the next read unless `allow_repaired`).
    pub fn was_repaired(&self) -> bool {
        self.pdf.was_repaired()
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
            .map(|s| decode_content(s))
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

fn decode_content(stream: &Stream<'_>) -> Result<ContentStream, ReaderError> {
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
    let data = stream
        .decoded_strict()
        .map_err(|_| ReaderError::DecodeFailed { obj })?
        .into_owned();
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
