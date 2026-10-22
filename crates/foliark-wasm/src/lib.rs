//! wasm-bindgen facade: a handle table over `foliark-engine` documents. One module per worker, single-threaded.
use foliark_engine::{Document, RenderOptions};
use foliark_model::Rect;
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

#[derive(Default)]
struct Docs {
    next: u32,
    open: HashMap<u32, Document>,
}

thread_local! {
    static DOCS: RefCell<Docs> = RefCell::default();
}

fn with_doc<T>(
    doc: u32,
    f: impl FnOnce(&Document) -> Result<T, foliark_model::PdfError>,
) -> Result<T, JsError> {
    DOCS.with(|d| match d.borrow().open.get(&doc) {
        Some(document) => f(document).map_err(|e| JsError::new(&e.to_string())),
        None => Err(JsError::new("unknown document handle")),
    })
}

/// Open a PDF and return a handle id.
#[wasm_bindgen]
pub fn open(bytes: Vec<u8>, password: Option<String>) -> Result<u32, JsError> {
    let document =
        Document::open(bytes, password.as_deref()).map_err(|e| JsError::new(&e.to_string()))?;
    DOCS.with(|d| {
        let mut d = d.borrow_mut();
        d.next += 1;
        let id = d.next;
        d.open.insert(id, document);
        Ok(id)
    })
}

#[wasm_bindgen(js_name = pageCount)]
pub fn page_count(doc: u32) -> Result<u32, JsError> {
    with_doc(doc, |d| Ok(d.page_count() as u32))
}

/// `PageGeometry` as a plain JS object (see packages/sdk/src/generated).
#[wasm_bindgen(js_name = pageGeometry)]
pub fn page_geometry(doc: u32, page: u32) -> Result<JsValue, JsError> {
    let geometry = with_doc(doc, |d| d.page_geometry(page as usize))?;
    serde_wasm_bindgen::to_value(&geometry).map_err(JsError::from)
}

#[wasm_bindgen]
pub struct RenderedPage(foliark_engine::RenderedPage);

#[wasm_bindgen]
impl RenderedPage {
    /// Everything but the pixels (spec 1.3): width, height, scale, clip, complete, warnings.
    #[wasm_bindgen(getter)]
    pub fn header(&self) -> Result<JsValue, JsError> {
        serde_wasm_bindgen::to_value(&self.0.header).map_err(JsError::from)
    }
    /// RGBA8 straight alpha, copied out of wasm memory. Moves the pixels out: a second call returns an empty array.
    #[wasm_bindgen(js_name = takeData)]
    pub fn take_data(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0.data)
    }
}

/// Spec 1.3 `RenderOptions` as sent by the SDK (`undefined` fields take the defaults).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Options {
    scale: Option<f64>,
    clip: Option<Rect>,
    annotations: Option<bool>,
    background: Option<[f64; 4]>,
}

#[wasm_bindgen(js_name = renderPage)]
pub fn render_page(doc: u32, page: u32, options: JsValue) -> Result<RenderedPage, JsError> {
    let o: Options = if options.is_undefined() {
        Options {
            scale: None,
            clip: None,
            annotations: None,
            background: None,
        }
    } else {
        serde_wasm_bindgen::from_value(options).map_err(JsError::from)?
    };
    let d = RenderOptions::default();
    let options = RenderOptions {
        scale: o.scale.unwrap_or(d.scale),
        clip: o.clip,
        annotations: o.annotations.unwrap_or(d.annotations),
        background: o.background.unwrap_or(d.background),
    };
    with_doc(doc, |d| d.render_page(page as usize, options)).map(RenderedPage)
}

/// Drop a document and its memory. Closing an unknown handle is a no-op.
#[wasm_bindgen]
pub fn close(doc: u32) {
    DOCS.with(|d| d.borrow_mut().open.remove(&doc));
}
