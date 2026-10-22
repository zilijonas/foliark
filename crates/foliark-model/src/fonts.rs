//! Font registration DTOs (spec §1.4) and the Rust error carried by every fallible op (spec §12).
use crate::{ErrorCode, ErrorDetails};
use serde::{Deserialize, Serialize};
use std::fmt;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FontStyle {
    Normal,
    Italic,
}

/// One face handed to `registerFonts`. `weight` is 400 or 700.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FontSource {
    #[ts(type = "ArrayBuffer | Uint8Array")]
    pub bytes: Vec<u8>,
    pub family: String,
    pub weight: u16,
    pub style: FontStyle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RegisteredFont {
    pub id: String,
    pub family: String,
    pub weight: u16,
    pub style: FontStyle,
    pub postscript_name: String,
    pub glyph_count: u32,
}

/// `pattern` is a lowercase substring of the normalized source font name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FontMatchRule {
    pub pattern: String,
    pub family: String,
}

/// `Result<T, PdfError>` is what every Rust op returns (spec §12/§13).
#[derive(Debug, Clone, PartialEq)]
pub struct PdfError {
    pub code: ErrorCode,
    /// Boxed: `ErrorDetails` is large and `Result<_, PdfError>` is everywhere.
    pub details: Option<Box<ErrorDetails>>,
}

impl PdfError {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            details: None,
        }
    }

    pub fn with(code: ErrorCode, details: ErrorDetails) -> Self {
        Self {
            code,
            details: Some(Box::new(details)),
        }
    }
}

impl fmt::Display for PdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.code)
    }
}

impl std::error::Error for PdfError {}
