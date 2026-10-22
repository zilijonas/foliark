//! The long-op table (spec §10.1, §13): a started op is a `Task` that the worker advances with
//! `step(budget)`. An op that finishes, fails or is cancelled is removed, and a `Task` changes
//! engine state only when it completes, so dropping one is the whole cancellation: nothing is
//! left behind. Between steps the worker yields and honours a cancel. A single step cannot be
//! interrupted (wasm is single-threaded), so the interpreter's work budget bounds it and the
//! client's hang watchdog ends one that does not return.
use crate::err;
use foliark_engine::{BoxTask, Engine, Step};
use foliark_model::{ErrorCode, PdfError};
use std::collections::HashMap;
use wasm_bindgen::JsValue;

/// A task with its output type erased to the JS value the worker receives.
pub type Erased = Box<dyn FnMut(&mut Engine, u64) -> Result<Step<JsValue>, JsValue>>;

pub fn erase<T: 'static>(
    mut task: BoxTask<T>,
    finish: fn(T) -> Result<JsValue, JsValue>,
) -> Erased {
    Box::new(move |e, budget| match task.step(e, budget).map_err(err)? {
        Step::Pending(p) => Ok(Step::Pending(p)),
        Step::Done(v) => finish(v).map(Step::Done),
    })
}

#[derive(Default)]
pub struct Operations {
    tasks: HashMap<u32, Erased>,
    next: u32,
}

impl Operations {
    pub fn start(&mut self, task: Erased) -> u32 {
        self.next += 1;
        self.tasks.insert(self.next, task);
        self.next
    }

    /// Run one step. Only a task that is still pending stays in the table.
    pub fn step(&mut self, e: &mut Engine, id: u32, budget: u64) -> Result<Step<JsValue>, JsValue> {
        let mut task = self
            .tasks
            .remove(&id)
            .ok_or_else(|| err(PdfError::new(ErrorCode::InvalidArgument)))?;
        let step = task(e, budget)?;
        if matches!(step, Step::Pending(_)) {
            self.tasks.insert(id, task);
        }
        Ok(step)
    }

    /// Tasks started and not finished, failed or cancelled.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn cancel(&mut self, id: u32) {
        self.tasks.remove(&id);
    }
}
