//! wasm-bindgen facade: `Engine` is `foliark_engine::Engine` 1:1 (spec §13). Arguments and
//! results cross as plain JS values via serde-wasm-bindgen; bytes out are owned copies, never
//! views into wasm memory. Errors are thrown as `{ code, message, details? }` objects.
//! Long ops are started (`startX` returns a task id) and driven with `step` so the worker can
//! yield and check cancellation between steps.
use foliark_engine::{RenderOptions, RenderedPage, Step};
use foliark_model::{
    ChangeSet, ExportMode, ExportPolicy, ExtractOptions, FoliarkError, FontMatchRule, FontSource,
    FontStyle, Limits, PdfError, Point, Rect, SearchOptions,
};
use js_sys::{Object, Reflect, Uint8Array};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

mod operation;
use operation::Operations;

fn to_js<T: Serialize>(v: &T) -> Result<JsValue, JsValue> {
    let s = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    v.serialize(&s).map_err(JsValue::from)
}

/// An argument; `undefined`/`null` take the type's default.
fn from_js<T: DeserializeOwned + Default>(v: JsValue) -> Result<T, JsValue> {
    if v.is_undefined() || v.is_null() {
        return Ok(T::default());
    }
    serde_wasm_bindgen::from_value(v).map_err(JsValue::from)
}

fn required<T: DeserializeOwned>(v: JsValue) -> Result<T, JsValue> {
    serde_wasm_bindgen::from_value(v).map_err(JsValue::from)
}

fn err(e: PdfError) -> JsValue {
    to_js(&FoliarkError::from(e)).unwrap_or_else(|e| e)
}

fn set(o: &Object, key: &str, v: JsValue) -> Result<(), JsValue> {
    Reflect::set(o, &key.into(), &v).map(|_| ())
}

/// Spec 1.3 `RenderedPage`: the header fields plus `data`, an owned RGBA8 copy.
fn rendered(page: RenderedPage) -> Result<JsValue, JsValue> {
    let out = to_js(&page.header)?;
    let obj: &Object = out.unchecked_ref();
    set(obj, "data", Uint8Array::from(&page.data[..]).into())?;
    Ok(out)
}

#[derive(Deserialize)]
struct FontMeta {
    family: String,
    weight: u16,
    style: FontStyle,
}

/// Spec 1.3 `RenderOptions` (`undefined` fields take the defaults).
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RenderArgs {
    scale: Option<f64>,
    clip: Option<Rect>,
    annotations: Option<bool>,
    background: Option<[f64; 4]>,
}

/// Linear memory the module has grown to (it never shrinks; a new worker returns it).
fn memory_bytes() -> f64 {
    #[cfg(target_arch = "wasm32")]
    return (core::arch::wasm32::memory_size(0) * 65536) as f64;
    #[cfg(not(target_arch = "wasm32"))]
    0.0
}

#[wasm_bindgen]
pub struct Engine {
    inner: foliark_engine::Engine,
    ops: Operations,
}

#[wasm_bindgen]
impl Engine {
    /// `limits`: a `Partial<Limits>` (or undefined); `generation`: the worker generation;
    /// `font_match`: `FontMatchRule[]` replacing the default table (or undefined).
    #[wasm_bindgen(constructor)]
    pub fn new(limits: JsValue, generation: u32, font_match: JsValue) -> Result<Engine, JsValue> {
        let limits: Limits = from_js(limits)?;
        let rules: Option<Vec<FontMatchRule>> = from_js(font_match)?;
        let mut inner = foliark_engine::Engine::new(limits).with_generation(generation);
        if let Some(rules) = rules {
            inner = inner.with_font_match(rules);
        }
        Ok(Engine {
            inner,
            ops: Operations::default(),
        })
    }

    #[wasm_bindgen(getter, js_name = fontRegistryVersion)]
    pub fn font_registry_version(&self) -> u32 {
        self.inner.font_registry_version()
    }

    /// `{ docId, info }`.
    pub fn open(
        &mut self,
        bytes: Vec<u8>,
        password: Option<String>,
        allow_repaired: Option<bool>,
    ) -> Result<JsValue, JsValue> {
        let (doc_id, info) = self
            .inner
            .open(bytes, password.as_deref(), allow_repaired.unwrap_or(false))
            .map_err(err)?;
        let out = Object::new();
        set(&out, "docId", doc_id.into())?;
        set(&out, "info", to_js(&info)?)?;
        Ok(out.into())
    }

    pub fn info(&mut self, doc: &str) -> Result<JsValue, JsValue> {
        to_js(&self.inner.info(doc).map_err(err)?)
    }

    pub fn pages(&mut self, doc: &str) -> Result<JsValue, JsValue> {
        to_js(&self.inner.pages(doc).map_err(err)?)
    }

    #[wasm_bindgen(js_name = hitTest)]
    pub fn hit_test(
        &mut self,
        doc: &str,
        page: u32,
        point: JsValue,
        tolerance: Option<f64>,
    ) -> Result<JsValue, JsValue> {
        let p: Point = required(point)?;
        to_js(&self.inner.hit_test(doc, page, p, tolerance).map_err(err)?)
    }

    pub fn select(
        &mut self,
        doc: &str,
        page: u32,
        from: JsValue,
        to: JsValue,
    ) -> Result<JsValue, JsValue> {
        let (from, to): (Point, Point) = (required(from)?, required(to)?);
        to_js(&self.inner.select(doc, page, from, to).map_err(err)?)
    }

    #[wasm_bindgen(js_name = quadsFor)]
    pub fn quads_for(
        &mut self,
        doc: &str,
        page: u32,
        start: u32,
        end: u32,
    ) -> Result<JsValue, JsValue> {
        to_js(&self.inner.quads_for(doc, page, start, end).map_err(err)?)
    }

    /// `fonts`: `{ bytes, family, weight, style }[]`; `bytes` is copied out of JS once.
    #[wasm_bindgen(js_name = registerFonts)]
    pub fn register_fonts(&mut self, fonts: js_sys::Array) -> Result<JsValue, JsValue> {
        let sources = fonts
            .iter()
            .map(|f| {
                let bytes = Uint8Array::new(&Reflect::get(&f, &"bytes".into())?).to_vec();
                let m: FontMeta = required(f)?;
                Ok(FontSource {
                    bytes,
                    family: m.family,
                    weight: m.weight,
                    style: m.style,
                })
            })
            .collect::<Result<Vec<_>, JsValue>>()?;
        to_js(&self.inner.register_fonts(sources).map_err(err)?)
    }

    /// One predefined-CMap pack file (spec 10.3), registered before the document that needs it is read.
    /// `ASSET_LOAD_FAILED` with `details.feature = "cmap:<name>"` when the bytes are not that CMap.
    #[wasm_bindgen(js_name = registerCMap)]
    pub fn register_cmap(&mut self, name: &str, bytes: &[u8]) -> Result<(), JsValue> {
        foliark_engine::cmaps::register(name, bytes).map_err(err)
    }

    /// Idempotent.
    pub fn close(&mut self, doc: &str) {
        self.inner.close(doc);
    }

    #[wasm_bindgen(js_name = startExtractText)]
    pub fn start_extract_text(
        &mut self,
        doc: &str,
        page: u32,
        options: JsValue,
    ) -> Result<u32, JsValue> {
        let o: ExtractOptions = from_js(options)?;
        let t = operation::erase(self.inner.extract_text(doc, page, o), |v| to_js(&v));
        Ok(self.ops.start(t))
    }

    #[wasm_bindgen(js_name = startSearch)]
    pub fn start_search(
        &mut self,
        doc: &str,
        query: &str,
        options: JsValue,
    ) -> Result<u32, JsValue> {
        let o: SearchOptions = from_js(options)?;
        let t = operation::erase(self.inner.search(doc, query, o), |v| to_js(&v));
        Ok(self.ops.start(t))
    }

    #[wasm_bindgen(js_name = startRender)]
    pub fn start_render(&mut self, doc: &str, page: u32, options: JsValue) -> Result<u32, JsValue> {
        let a: RenderArgs = from_js(options)?;
        let d = RenderOptions::default();
        let o = RenderOptions {
            scale: a.scale.unwrap_or(d.scale),
            clip: a.clip,
            annotations: a.annotations.unwrap_or(d.annotations),
            background: a.background.unwrap_or(d.background),
        };
        let t = operation::erase(self.inner.render(doc, page, o), rendered);
        Ok(self.ops.start(t))
    }

    #[wasm_bindgen(js_name = startCapabilities)]
    pub fn start_capabilities(&mut self, doc: &str, deep: Option<bool>) -> u32 {
        let t = operation::erase(self.inner.capabilities(doc, deep.unwrap_or(false)), |v| {
            to_js(&v)
        });
        self.ops.start(t)
    }

    #[wasm_bindgen(js_name = startApplyChanges)]
    pub fn start_apply_changes(&mut self, doc: &str, changes: JsValue) -> Result<u32, JsValue> {
        let cs: ChangeSet = required(changes)?;
        let t = operation::erase(self.inner.apply_changes(doc, cs), |v| to_js(&v));
        Ok(self.ops.start(t))
    }

    #[wasm_bindgen(js_name = startPrepareExport)]
    pub fn start_prepare_export(
        &mut self,
        doc: &str,
        mode: JsValue,
        policy: JsValue,
    ) -> Result<u32, JsValue> {
        let (mode, policy): (ExportMode, ExportPolicy) = (required(mode)?, from_js(policy)?);
        let t = operation::erase(self.inner.prepare_export(doc, mode, policy), |v| to_js(&v));
        Ok(self.ops.start(t))
    }

    #[wasm_bindgen(js_name = prepareParagraph)]
    pub fn prepare_paragraph(
        &mut self,
        doc: &str,
        page: u32,
        rects: JsValue,
        font: Option<String>,
        editor_face: Option<bool>,
    ) -> Result<JsValue, JsValue> {
        let rects: Vec<Rect> = required(rects)?;
        let p = self
            .inner
            .prepare_paragraph(doc, page, rects, font, editor_face.unwrap_or(false))
            .map_err(err)?;
        let out = to_js(&p)?;
        if let Some(face) = &p.editor_face {
            // `bytes` is typed `Uint8Array` (and transferred), not the number array serde makes.
            let f = Reflect::get(&out, &"editorFace".into())?;
            set(
                f.unchecked_ref(),
                "bytes",
                Uint8Array::from(&face.bytes[..]).into(),
            )?;
        }
        Ok(out)
    }

    /// ADR 0004: the layout an edit would commit, nothing committed.
    #[wasm_bindgen(js_name = layoutEdit)]
    pub fn layout_edit(
        &mut self,
        doc: &str,
        page: u32,
        rects: JsValue,
        text: &str,
        font: Option<String>,
        glyphs: Option<bool>,
    ) -> Result<JsValue, JsValue> {
        let rects: Vec<Rect> = required(rects)?;
        let p = self
            .inner
            .layout_edit(doc, page, rects, text, font, glyphs.unwrap_or(false))
            .map_err(err)?;
        to_js(&p)
    }

    /// The export bytes as an owned copy; frees the plan.
    #[wasm_bindgen(js_name = takeExport)]
    pub fn take_export(&mut self, doc: &str, plan_id: &str) -> Result<Vec<u8>, JsValue> {
        self.inner.take_export(doc, plan_id).map_err(err)
    }

    /// Advance a task by about `budget` work units: `{ done: false, progress }` or
    /// `{ done: true, result }`. A task that fails, finishes or is cancelled is gone.
    pub fn step(&mut self, task: u32, budget: f64) -> Result<JsValue, JsValue> {
        let out = Object::new();
        match self.ops.step(&mut self.inner, task, budget as u64)? {
            Step::Pending(p) => {
                set(&out, "done", false.into())?;
                set(&out, "progress", to_js(&p)?)?;
            }
            Step::Done(v) => {
                set(&out, "done", true.into())?;
                set(&out, "result", v)?;
            }
        }
        Ok(out.into())
    }

    /// Diagnostics for leak checks, no document content: what is held right now.
    pub fn stats(&self) -> Result<JsValue, JsValue> {
        let s = self.inner.stats();
        let out = Object::new();
        for (key, v) in [
            ("documents", s.documents as f64),
            ("sourceBytes", s.source_bytes as f64),
            ("cacheEntries", s.cache_entries as f64),
            ("cacheBytes", s.cache_bytes as f64),
            ("cacheEvictions", s.cache_evictions as f64),
            ("tasks", self.ops.len() as f64),
            ("memoryBytes", memory_bytes()),
        ] {
            set(&out, key, v.into())?;
        }
        Ok(out.into())
    }

    /// Drop a task without finishing it. Unknown ids are ignored.
    pub fn cancel(&mut self, task: u32) {
        self.ops.cancel(task);
    }
}
