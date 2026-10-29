//! Public contract DTOs (spec §1, §4, §9 to §12). Rust is the single source: serde camelCase on the wire,
//! ts-rs generates `packages/sdk/src/generated/` (never hand-edit; `scripts/check-ts-parity.sh` guards drift).
//! Document text never appears in errors except `ErrorDetails::chars` (spec §12).
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use ts_rs::TS;

/// A constant that serializes as one JSON literal and rejects any other value, so an
/// envelope like `{ v: 1, ok: true }` is checked by serde and typed as a TS literal.
macro_rules! literal {
    ($name:ident, $ty:ty, $val:expr, $ts:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, TS)]
        #[ts(type = $ts)]
        pub struct $name;
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                <$ty>::serialize(&$val, s)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                if <$ty>::deserialize(d)? == $val {
                    Ok(Self)
                } else {
                    Err(D::Error::custom(concat!("expected ", $ts)))
                }
            }
        }
    };
}
literal!(V1, u8, 1, "1");
literal!(OkTrue, bool, true, "true");
literal!(OkFalse, bool, false, "false");

// ---- §12 errors ---------------------------------------------------------------------------------

/// Every typed refusal or failure of the public API (spec §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export)]
pub enum ErrorCode {
    InvalidArgument,
    MalformedPdf,
    RepairedPdf,
    DecodeFailed,
    PasswordRequired,
    PasswordIncorrect,
    EncryptionUnsupported,
    LimitExceeded,
    OutOfMemory,
    AssetLoadFailed,
    FontUnsupported,
    EditNoStyle,
    EditRotatedText,
    EditNoFont,
    EditNoFit,
    EditUnsupportedScript,
    StaleRevision,
    StalePlan,
    PlanConsumed,
    FormAppearanceMissing,
    XfaUnsupported,
    OptionalContentUnsupported,
    SignedInput,
    RedactPatternUnsupported,
    RedactVectorUnsupported,
    RedactVerifyFailed,
    RenderIncomplete,
    ConsentRequired,
    Cancelled,
    WorkerReset,
    DocumentGone,
    DocumentClosed,
    ClientDisposed,
    ProtocolError,
    Internal,
}

/// Structured error context. Only `chars` (of a refused edit) may contain document text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export, optional_fields)]
pub struct ErrorDetails {
    pub limit: Option<String>,
    pub actual: Option<f64>,
    pub max: Option<f64>,
    pub feature: Option<String>,
    pub pages: Option<Vec<u32>>,
    pub fields: Option<Vec<String>>,
    pub edit_id: Option<String>,
    pub chars: Option<String>,
    pub needed_lines: Option<u32>,
    pub max_lines: Option<u32>,
    pub obj_ref: Option<String>,
    /// Ids of the refused operations (redactions) the error is about.
    pub ids: Option<Vec<String>>,
}

/// The error a Rust op returns across the wasm boundary (`PdfError` plus a message).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, optional_fields)]
pub struct FoliarkError {
    pub code: ErrorCode,
    pub message: String,
    pub details: Option<ErrorDetails>,
}

/// `FoliarkError` as it travels in a protocol response (spec §10.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ErrorWire {
    #[serde(flatten)]
    pub error: FoliarkError,
    pub recoverable: bool,
}

// ---- §11 capabilities ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CapabilityStatus {
    Supported,
    Unsupported,
    /// Not yet established (spec §11).
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, optional_fields)]
pub struct CapabilityReason {
    pub code: ErrorCode,
    pub pages: Option<Vec<u32>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Capability {
    pub status: CapabilityStatus,
    /// `unsupported` always carries the code the op would return.
    pub reasons: Vec<CapabilityReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Capabilities {
    pub extract: Capability,
    pub render: Capability,
    pub edit: Capability,
    pub redact: Capability,
    pub export_native: Capability,
    pub export_raster: Capability,
}

// ---- §9 limits ----------------------------------------------------------------------------------

/// Resource limits. Deserializing a partial object fills the rest with the SDK defaults, so the
/// facade can take `Partial<Limits>` straight from `FoliarkOptions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Limits {
    pub max_input_bytes: u32,
    pub max_pages: u32,
    pub max_objects: u32,
    pub max_stream_decoded_bytes: u32,
    pub max_decode_ratio: u32,
    pub max_image_pixels: u32,
    pub max_render_pixels: u32,
    pub max_nesting: u32,
    #[ts(type = "number")]
    pub max_work_units: u64,
    pub max_output_bytes: u32,
    pub max_open_documents: u32,
    pub max_registered_font_bytes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        const MIB: u32 = 1 << 20;
        Self {
            max_input_bytes: 64 * MIB,
            max_pages: 1000,
            max_objects: 1_000_000,
            max_stream_decoded_bytes: 128 * MIB,
            max_decode_ratio: 1000,
            max_image_pixels: 100_000_000,
            max_render_pixels: 50_000_000,
            max_nesting: 32,
            max_work_units: 5_000_000_000,
            max_output_bytes: 256 * MIB,
            max_open_documents: 4,
            max_registered_font_bytes: 64 * MIB,
        }
    }
}

// ---- §1.1 geometry and info ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Top-left `x`, `y` (HDDN BoundingBox field names).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Glyph-oriented corners: top-left, top-right, bottom-right, bottom-left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Quad(pub [Point; 4]);

/// `x' = a x + c y + e`, `y' = b x + d y + f`, stored `[a, b, c, d, e, f]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, type = "[number, number, number, number, number, number]")]
pub struct Matrix(pub [f64; 6]);

impl Matrix {
    /// `None` when the matrix is singular. Negative zeros are normalized to +0 (`x + 0.0`).
    pub fn inverse(self) -> Option<Self> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        (det != 0.0 && det.is_finite()).then(|| {
            let m = [d, -b, -c, a, c * f - d * e, b * e - a * f];
            Self(m.map(|x| x / det + 0.0))
        })
    }
}

/// Display-space size and boxes of a page (spec §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PageGeometry {
    pub index: u32,
    pub width: f64,
    pub height: f64,
    /// Clockwise /Rotate in degrees.
    #[ts(type = "0 | 90 | 180 | 270")]
    pub rotation: u16,
    pub user_unit: f64,
    /// User space `[x0, y0, x1, y1]`.
    #[ts(type = "[number, number, number, number]")]
    pub media_box: [f64; 4],
    /// User space, already intersected with the media box.
    #[ts(type = "[number, number, number, number]")]
    pub crop_box: [f64; 4],
    pub user_to_display: Matrix,
    pub display_to_user: Matrix,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocumentFeatures {
    pub forms: bool,
    pub xfa: bool,
    pub signatures: bool,
    pub optional_content: bool,
    pub javascript: bool,
    pub embedded_files: bool,
    pub annotations: u32,
    pub struct_tree: bool,
    pub outlines: bool,
    pub thumbnails: bool,
    pub metadata: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocumentInfo {
    pub page_count: u32,
    pub pdf_version: String,
    pub encrypted: bool,
    pub repaired: bool,
    pub features: DocumentFeatures,
    /// e.g. `cmap:UniJIS-UCS2-H`.
    pub required_assets: Vec<String>,
}

// ---- §1.2 text ----------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WordSource {
    Native,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, optional_fields)]
pub struct Word {
    pub text: String,
    /// Half-open UTF-16 offsets into `TextPage::text`.
    pub start: u32,
    pub end: u32,
    /// Axis-aligned union of cluster quads.
    #[serde(rename = "box")]
    pub bounds: Rect,
    pub block_index: u32,
    pub line_index: u32,
    pub word_index: u32,
    /// Style of the first cluster.
    pub style_index: u32,
    /// All clusters render mode 3 or 7.
    pub invisible: bool,
    #[ts(type = "true", optional)]
    pub ocg_hidden: Option<bool>,
    pub source: WordSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Cluster {
    pub start: u32,
    pub end: u32,
    pub quad: Quad,
    pub style_index: u32,
    pub glyphs: u32,
    /// 1 invisible, 2 unmapped, 4 multiCodepoint, 8 ocgHidden.
    pub flags: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StyleFlags {
    pub bold: bool,
    pub italic: bool,
    pub serif: bool,
    pub mono: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StyleRun {
    /// Subset tag stripped.
    pub font_name: String,
    /// Object reference, e.g. `12 0`.
    pub font_ref: String,
    pub size: f64,
    /// sRGB 0..1.
    #[ts(type = "[number, number, number]")]
    pub color: [f64; 3],
    pub render_mode: u8,
    pub flags: StyleFlags,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TextStats {
    pub visible_clusters: u32,
    pub invisible_clusters: u32,
    pub unmapped_glyphs: u32,
    /// Largest single image occurrence bbox intersected with the page, over page area.
    pub image_coverage: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, optional_fields)]
pub struct TextPage {
    pub doc_id: String,
    pub revision: u32,
    pub page_index: u32,
    pub text: String,
    pub words: Vec<Word>,
    pub styles: Vec<StyleRun>,
    /// Only with `chars: true`, in text order.
    pub clusters: Option<Vec<Cluster>>,
    pub stats: TextStats,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Hit {
    pub start: u32,
    pub end: u32,
    pub word_index: u32,
    pub quad: Quad,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TextRange {
    pub start: u32,
    pub end: u32,
    pub text: String,
    /// One per line.
    pub quads: Vec<Quad>,
    pub partial_cluster: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchHit {
    #[serde(flatten)]
    pub range: TextRange,
    pub page_index: u32,
    pub revision: u32,
}

// ---- §1.3 render --------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export)]
pub enum RenderWarningCode {
    FontSubstituted,
    ImageDecodeFailed,
    AnnotationAppearanceMissing,
    CmapMissing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RenderWarning {
    pub code: RenderWarningCode,
    pub detail: String,
}

/// Everything of a `RenderedPage` except the pixels: `data` (RGBA8 straight alpha) travels as a
/// transferred `ArrayBuffer` of exactly `width * height * 4` bytes (spec §10.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RenderedPageHeader {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub clip: Rect,
    /// False iff any warning.
    pub complete: bool,
    pub warnings: Vec<RenderWarning>,
}

// ---- §1.5 paragraphs and changes ----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FontRule {
    Caller,
    NameMatch,
    Flags,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "source",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum FontResolution {
    Embedded {
        font_ref: String,
        name: String,
        covers_ascii: bool,
    },
    Registered {
        font_id: String,
        family: String,
        rule: FontRule,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EditorFace {
    pub font_key: String,
    /// Travels as a transferred `ArrayBuffer`, never a view into wasm memory.
    #[ts(type = "Uint8Array")]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ParagraphInfo {
    pub doc_id: String,
    pub revision: u32,
    pub page_index: u32,
    pub bounds: Rect,
    pub origin_x: f64,
    pub top: f64,
    pub first_baseline: f64,
    pub font_size: f64,
    pub line_step: f64,
    #[ts(type = "[number, number, number]")]
    pub color: [f64; 3],
    pub bold: bool,
    pub italic: bool,
    pub serif: bool,
    pub mono: bool,
    pub mixed_style: bool,
    pub wrap_width: f64,
    pub max_height: f64,
    pub max_lines: u32,
    pub original_text: Option<String>,
    pub font: FontResolution,
    #[ts(optional = nullable)]
    pub editor_face: Option<EditorFace>,
    /// ADR 0012's style segments, top to bottom; empty when `originalText` is null. Their ranges
    /// tile `originalText` exactly (a segment's hard-break LF ends it), so slicing the text at
    /// them and joining the slices gives it back. Each describes the page's own font, never the
    /// caller's.
    pub segments: Vec<ParagraphSegment>,
    /// The page styles the selection is drawn in (ADR 0013), each with the page's own font
    /// (never the caller's). `runs` and `EditPreview.styles` index them.
    pub looks: Vec<TextLook>,
    /// Style runs of `originalText`: UTF-16 ranges that tile it, each drawn in one look. Empty
    /// when `originalText` is null.
    pub runs: Vec<StyleSpan>,
}

/// One page style of a paragraph (ADR 0013), as `ParagraphSegment` describes a segment's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TextLook {
    pub font_size: f64,
    #[ts(type = "[number, number, number]")]
    pub color: [f64; 3],
    pub bold: bool,
    pub italic: bool,
    pub serif: bool,
    pub mono: bool,
    pub font: FontResolution,
    #[ts(optional = nullable)]
    pub editor_face: Option<EditorFace>,
}

/// A UTF-16 range of a paragraph's text drawn in `ParagraphInfo.looks[look]` (ADR 0013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StyleSpan {
    pub start: u32,
    pub end: u32,
    pub look: u32,
}

/// One style segment of a paragraph: consecutive hard lines drawn in one look (ADR 0012).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ParagraphSegment {
    /// UTF-16 offsets into `ParagraphInfo.originalText`, `start` inclusive.
    pub start: u32,
    pub end: u32,
    pub font_size: f64,
    #[ts(type = "[number, number, number]")]
    pub color: [f64; 3],
    pub bold: bool,
    pub italic: bool,
    pub serif: bool,
    pub mono: bool,
    pub font: FontResolution,
    #[ts(optional = nullable)]
    pub editor_face: Option<EditorFace>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, optional_fields)]
pub struct TextEdit {
    pub id: String,
    pub page_index: u32,
    pub rects: Vec<Rect>,
    pub text: String,
    /// A registered `fontId`.
    pub font: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum Redaction {
    Region {
        id: String,
        page_index: u32,
        rect: Rect,
        /// Omitted = black; an explicit `null` = no fill (the content is still removed).
        #[serde(
            default,
            with = "fill_or_none",
            skip_serializing_if = "Option::is_none"
        )]
        #[ts(type = "[number, number, number] | null", optional)]
        fill: Option<Option<[f64; 3]>>,
    },
    Text {
        id: String,
        page_index: u32,
        start: u32,
        end: u32,
        #[serde(
            default,
            with = "fill_or_none",
            skip_serializing_if = "Option::is_none"
        )]
        #[ts(type = "[number, number, number] | null", optional)]
        fill: Option<Option<[f64; 3]>>,
    },
}

/// Tells an omitted `fill` (`None`, black) from an explicit `null` (`Some(None)`, no fill).
mod fill_or_none {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Option<[f64; 3]>>, D::Error> {
        <Option<[f64; 3]>>::deserialize(d).map(Some)
    }

    pub fn serialize<S: Serializer>(v: &Option<Option<[f64; 3]>>, s: S) -> Result<S::Ok, S::Error> {
        v.flatten().serialize(s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, optional_fields)]
pub struct ChangeSet {
    /// Not equal to the handle's revision -> `STALE_REVISION`.
    pub base_revision: u32,
    /// Present replaces the whole edit list.
    pub text_edits: Option<Vec<TextEdit>>,
    /// Present replaces the whole redaction list.
    pub redactions: Option<Vec<Redaction>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, optional_fields)]
pub struct RejectedChange {
    pub id: String,
    pub code: ErrorCode,
    pub details: Option<ErrorDetails>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RunSource {
    Embedded,
    Registered,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LayoutRun {
    pub source: RunSource,
    pub font: String,
    pub clusters: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EditLayout {
    pub id: String,
    pub lines: u32,
    pub approximate: bool,
    /// Hash of the layout inputs; a preview (ADR 0004) with the same inputs has the same one.
    pub fingerprint: String,
    pub runs: Vec<LayoutRun>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChangeResult {
    pub accepted: bool,
    /// Unchanged if not accepted.
    pub revision: u32,
    pub rejected: Vec<RejectedChange>,
    pub changed_pages: Vec<u32>,
    pub layouts: Vec<EditLayout>,
}

/// One line of an [`EditPreview`]: `text` is the normalized text it shows, `x` the origin of its
/// first glyph and `baseline` its baseline, in display points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PreviewLine {
    pub text: String,
    pub baseline: f64,
    pub x: f64,
    pub width: f64,
}

/// A positioned glyph in display space: its pen starts at `x` on line `line`, spans `width`, and
/// stands for UTF-16 offsets `start..end` of the normalized text (a ligature spans several).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PreviewGlyph {
    pub line: u32,
    pub font: String,
    pub gid: u32,
    pub start: u32,
    pub end: u32,
    pub x: f64,
    pub width: f64,
}

/// ADR 0004: the layout `applyChanges` would commit for one text edit, without committing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, optional_fields)]
pub struct EditPreview {
    pub doc_id: String,
    /// The revision it was laid out on; stale once the document's revision differs.
    pub revision: u32,
    /// Stale once the font registry version differs.
    pub font_registry_version: u32,
    pub font_size: f64,
    pub lines: Vec<PreviewLine>,
    pub needed_lines: u32,
    pub max_lines: u32,
    /// False: `applyChanges` would refuse with `EDIT_NO_FIT`.
    pub fits: bool,
    pub approximate: bool,
    pub runs: Vec<LayoutRun>,
    /// Equals `ChangeResult.layouts[].fingerprint` for the same inputs.
    pub fingerprint: String,
    /// Style runs of the whole normalized text sent (ADR 0013): UTF-16 ranges that tile it, each
    /// indexing the `looks` `prepareParagraph` returns for the same rects.
    pub styles: Vec<StyleSpan>,
    /// Only when asked for.
    pub glyphs: Option<Vec<PreviewGlyph>>,
}

// ---- §1.6 export --------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExportMode {
    Native,
    Raster,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AnnotationPolicy {
    #[default]
    Flatten,
    Remove,
}

/// Native export only (ADR 0014): `Keep` writes a hyperlink back, fresh from an allowlist, when it
/// cannot reveal redacted content; raster export removes every link either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LinkPolicy {
    #[default]
    Remove,
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SignedInputPolicy {
    #[default]
    Strip,
    Refuse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum InvisibleTextPolicy {
    #[default]
    Keep,
    Remove,
    /// Keep invisible text only where it lies over a drawn raster image (a scanner's OCR
    /// layer); every other invisible show is removed and counted (ADR 0008).
    KeepOcr,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct RasterPolicy {
    pub acknowledge_losses: bool,
    pub scale: f64,
}

impl Default for RasterPolicy {
    fn default() -> Self {
        Self {
            acknowledge_losses: false,
            scale: 2.0,
        }
    }
}

/// Spec §1.6 policy. The SDK sends `Partial<ExportPolicy>`: missing fields take these defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ExportPolicy {
    pub annotations: AnnotationPolicy,
    pub links: LinkPolicy,
    pub signed_input: SignedInputPolicy,
    pub invisible_text: InvisibleTextPolicy,
    pub raster: RasterPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SanitizedItem {
    Metadata,
    EmbeddedFiles,
    Actions,
    Annotations,
    Widgets,
    Thumbnails,
    Structure,
    Outlines,
    PageLabels,
    OffPage,
    InvisibleText,
    OcgContent,
    UnusedResources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export)]
pub enum LossCode {
    MetadataRemoved,
    AttachmentsRemoved,
    ScriptsRemoved,
    AnnotationsFlattened,
    AnnotationsRemoved,
    LinksRemoved,
    FormsFlattened,
    LayersFlattened,
    AccessibilityRemoved,
    OutlinesRemoved,
    PageLabelsRemoved,
    PdfaClaimRemoved,
    EncryptionRemoved,
    SignaturesInvalidated,
    ImageReencoded,
    VectorCollateral,
    VectorCutApproximated,
    FontSubstituted,
    FontGlyphInventoryRetained,
    TextSearchLost,
    VectorRasterized,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Flattened {
    pub annotations: u32,
    pub widgets: u32,
    pub ocg_layers: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Collateral {
    pub page_index: u32,
    pub bounds: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SharedKind {
    Image,
    Form,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SharedSurvivor {
    pub kind: SharedKind,
    pub redacted_on: Vec<u32>,
    pub untouched_on: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedactionReport {
    pub regions: u32,
    pub clusters_removed: u32,
    pub images_erased: u32,
    pub images_cloned: u32,
    pub forms_cloned: u32,
    pub vector_subpaths_removed: u32,
    pub vector_paths_cut: u32,
    /// Spec §5.4c.
    pub collateral: Vec<Collateral>,
    pub shared_survivors: Vec<SharedSurvivor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EditsReport {
    pub applied: u32,
    pub approximate: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KeptReport {
    pub invisible_text_clusters: u32,
    /// Hyperlinks written back under `policy.links: 'keep'` (ADR 0014); the rest are in
    /// `LINKS_REMOVED`.
    pub links: u32,
    pub fonts_with_glyph_inventory: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, optional_fields)]
pub struct Loss {
    pub code: LossCode,
    pub count: u32,
    pub pages: Option<Vec<u32>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportReport {
    pub mode: ExportMode,
    pub pages: u32,
    pub output_bytes: u32,
    pub removed: BTreeMap<SanitizedItem, u32>,
    pub flattened: Flattened,
    pub redaction: RedactionReport,
    pub edits: EditsReport,
    pub kept: KeptReport,
    pub losses: Vec<Loss>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportPlan {
    pub plan_id: String,
    pub doc_id: String,
    pub generation: u32,
    pub revision: u32,
    pub mode: ExportMode,
    pub policy_hash: String,
    pub font_registry_version: u32,
    pub byte_length: u32,
    pub report: ExportReport,
}

// ---- §10.4 protocol envelope --------------------------------------------------------------------

/// One entry per public op, plus `init` (carries `wasmUrl`, starts the generation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum OpName {
    Init,
    Open,
    Info,
    Pages,
    Capabilities,
    ExtractText,
    HitTest,
    Select,
    QuadsFor,
    Search,
    RenderPage,
    PrepareParagraph,
    LayoutEdit,
    ApplyChanges,
    PrepareExport,
    Export,
    RegisterFonts,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Phase {
    Open,
    Extract,
    Render,
    Layout,
    Export,
    Verify,
}

/// Main thread to worker request. `gen` is the sender's worker generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Req<A> {
    pub v: V1,
    pub id: u32,
    #[serde(rename = "gen")]
    pub generation: u32,
    pub op: OpName,
    #[ts(optional)]
    pub doc: Option<String>,
    pub args: A,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResOk<R> {
    pub v: V1,
    pub id: u32,
    pub ok: OkTrue,
    pub result: R,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResErr {
    pub v: V1,
    pub id: u32,
    pub ok: OkFalse,
    pub error: ErrorWire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
#[allow(clippy::large_enum_variant)] // a wire envelope, built once per message
pub enum Res<R> {
    Ok(ResOk<R>),
    Err(ResErr),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Progress {
    pub phase: Phase,
    pub done: u32,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Prog {
    pub v: V1,
    pub id: u32,
    pub progress: Progress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CtlEvent {
    Ready,
    Fatal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CtlCancel {
    pub v: V1,
    /// Request id to cancel; a queued op is dropped before it starts.
    pub cancel: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, optional_fields)]
pub struct CtlEventMsg {
    pub v: V1,
    pub event: CtlEvent,
    #[serde(rename = "gen")]
    pub generation: u32,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum Ctl {
    Cancel(CtlCancel),
    Event(CtlEventMsg),
}

#[cfg(test)]
mod tests {
    use super::Matrix;

    #[test]
    fn inverse_undoes_a_rotated_translated_page_transform() {
        // Rotate 90, flip y, translate: what a rotated, offset CropBox produces.
        let m = Matrix([0.0, 1.0, 1.0, 0.0, -5.0, 7.0]);
        let [a, b, c, d, e, f] = m.0;
        let [ia, ib, ic, id, ie, if_] = m.inverse().unwrap().0;
        // m * inverse = identity, composed as affine maps (x' = a x + c y + e).
        let got = [
            a * ia + c * ib,
            b * ia + d * ib,
            a * ic + c * id,
            b * ic + d * id,
            a * ie + c * if_ + e,
            b * ie + d * if_ + f,
        ];
        assert_eq!(got, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(Matrix([1.0, 2.0, 2.0, 4.0, 0.0, 0.0]).inverse(), None);
    }
}
