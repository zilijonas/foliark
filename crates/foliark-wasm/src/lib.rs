//! wasm-bindgen facade: a handle table over `foliark-engine` documents. One module per worker, single-threaded.
use foliark_engine::Document;
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
    f: impl FnOnce(&Document) -> Result<T, foliark_engine::EngineError>,
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
pub struct RenderedPage {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

#[wasm_bindgen]
impl RenderedPage {
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.width
    }
    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.height
    }
    /// RGBA8 straight alpha, copied out of wasm memory.
    #[wasm_bindgen(getter)]
    pub fn data(&self) -> Vec<u8> {
        self.data.clone()
    }
}

#[wasm_bindgen(js_name = renderPage)]
pub fn render_page(doc: u32, page: u32, scale: f64) -> Result<RenderedPage, JsError> {
    let r = with_doc(doc, |d| d.render_page(page as usize, scale))?;
    Ok(RenderedPage {
        width: r.width,
        height: r.height,
        data: r.data,
    })
}

/// Drop a document and its memory. Closing an unknown handle is a no-op.
#[wasm_bindgen]
pub fn close(doc: u32) {
    DOCS.with(|d| d.borrow_mut().open.remove(&doc));
}
