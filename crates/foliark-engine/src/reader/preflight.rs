//! Preflight: a cheap inventory of what a file contains, before any extraction. Reads
//! dictionaries only; actions are never executed and signatures are only reported.
use super::{Document, ReaderError, object_id};
use crate::limits::{self, Budget};
use foliark_model::{DocumentFeatures, DocumentInfo};
use hayro::hayro_syntax::object::stream::Stream;
use hayro::hayro_syntax::object::{Array, Dict, Name, Object};
use std::collections::BTreeSet;

fn name_is(d: &Dict<'_>, key: &str, value: &str) -> bool {
    d.get::<Name<'_>>(key).is_some_and(|n| n.as_str() == value)
}

fn is_js_action(d: &Dict<'_>) -> bool {
    name_is(d, "S", "JavaScript") || d.contains_key("JS")
}

/// Codec streams are decoded by the image path, which the pixel limit guards.
fn is_image_codec(s: &Stream<'_>) -> bool {
    use hayro::hayro_syntax::Filter::*;
    s.filters()
        .iter()
        .any(|f| matches!(f, DctDecode | JpxDecode | CcittFaxDecode | Jbig2Decode))
}

#[derive(Default)]
struct Scan {
    signatures: bool,
    javascript: bool,
    embedded_files: bool,
    assets: BTreeSet<String>,
}

impl Scan {
    fn dict(&mut self, d: &Dict<'_>) {
        // A populated signature value: /ByteRange with the signed /Contents.
        self.signatures |= d.contains_key("ByteRange") && d.contains_key("Contents");
        self.signatures |= name_is(d, "FT", "Sig") && d.get::<Dict<'_>>("V").is_some();
        self.javascript |= is_js_action(d)
            || d.contains_key("AA")
            || d.get::<Dict<'_>>("A").is_some_and(|a| is_js_action(&a));
        self.embedded_files |= d.contains_key("EF") || name_is(d, "Subtype", "FileAttachment");
        // Predefined CMaps (not Identity) are fetched from the optional CMap pack (spec §10.3).
        for key in ["Encoding", "UseCMap"] {
            if let Some(n) = d.get::<Name<'_>>(key) {
                let is_cmap_font = name_is(d, "Subtype", "Type0") || key == "UseCMap";
                if is_cmap_font && !n.as_str().starts_with("Identity") {
                    self.assets.insert(format!("cmap:{}", n.as_str()));
                }
            }
        }
    }
}

impl Document {
    /// The spec §1.1 `DocumentInfo`. Also checks every stream and image of the file against the
    /// budgets (spec §9) without keeping what it decodes, so a bomb is refused here and not
    /// while rendering. A stream that does not decode is only an error when it is read.
    pub fn info(&self) -> Result<DocumentInfo, ReaderError> {
        let pdf = &self.pdf;
        let mut budget = Budget::new(&self.limits);
        let mut scan = Scan::default();
        for object in pdf.objects() {
            match object {
                Object::Dict(d) => scan.dict(&d),
                Object::Stream(s) => {
                    let d = s.dict();
                    scan.dict(d);
                    let obj = object_id(s.obj_id().into());
                    if name_is(d, "Subtype", "Image") {
                        let dim = |k| d.get::<u32>(k).or_else(|| d.get::<u32>("W")).unwrap_or(0);
                        let (w, h) = (dim("Width"), d.get::<u32>("Height").unwrap_or(0));
                        budget.charge_pixels(limits::image_pixels(&self.limits, w, h)?)?;
                    }
                    if !is_image_codec(&s) {
                        match limits::decode_stream(&s, obj, &self.limits, &mut budget) {
                            Err(ReaderError::DecodeFailed { .. }) | Ok(_) => {}
                            Err(e) => return Err(e),
                        }
                    }
                }
                _ => {}
            }
        }

        let catalog = pdf
            .xref()
            .get::<Dict<'_>>(pdf.xref().root_id())
            .ok_or(ReaderError::MalformedPdf)?;
        let names = catalog.get::<Dict<'_>>("Names");
        let in_names = |k: &str| names.as_ref().is_some_and(|n| n.contains_key(k));
        let acro = catalog.get::<Dict<'_>>("AcroForm");
        let perms_doc_mdp = catalog
            .get::<Dict<'_>>("Perms")
            .is_some_and(|p| p.contains_key("DocMDP"));
        let (mut annotations, mut thumbnails) = (0u32, false);
        for page in pdf.pages().iter() {
            let raw = page.raw();
            let n = raw
                .get::<Array<'_>>("Annots")
                .map_or(0, |a| a.raw_iter().count());
            annotations = annotations.saturating_add(u32::try_from(n).unwrap_or(u32::MAX));
            thumbnails |= raw.contains_key("Thumb");
        }
        let features = DocumentFeatures {
            forms: acro
                .as_ref()
                .and_then(|a| a.get::<Array<'_>>("Fields"))
                .is_some_and(|f| f.raw_iter().next().is_some()),
            xfa: acro.as_ref().is_some_and(|a| a.contains_key("XFA")),
            signatures: scan.signatures || perms_doc_mdp,
            optional_content: catalog.contains_key("OCProperties"),
            javascript: scan.javascript
                || in_names("JavaScript")
                || catalog
                    .get::<Dict<'_>>("OpenAction")
                    .is_some_and(|a| is_js_action(&a)),
            embedded_files: scan.embedded_files
                || in_names("EmbeddedFiles")
                || catalog.contains_key("AF"),
            annotations,
            struct_tree: catalog.contains_key("StructTreeRoot"),
            outlines: catalog.contains_key("Outlines"),
            thumbnails,
            metadata: catalog.contains_key("Metadata") || *pdf.metadata() != Default::default(),
        };
        // Repair is lazy: ask after everything was read.
        self.check_repaired()?;
        let version = format!("{:?}", pdf.version());
        Ok(DocumentInfo {
            page_count: pdf.pages().len() as u32,
            pdf_version: format!("{}.{}", &version[3..4], &version[4..5]),
            encrypted: pdf.is_encrypted(),
            repaired: self.was_repaired(),
            features,
            required_assets: scan.assets.into_iter().collect(),
        })
    }
}
