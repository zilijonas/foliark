//! Raster export job (spec §7, §10.1): [`RasterPlan`] once, then one page per unit of work
//! (draw, burn, encode, drop the pixels), so a worker can yield and cancel between pages and
//! only one page buffer is ever alive. A cancel, a failure or a refusal leaves no output.
//!
//! A page that draws with a warning is not encoded; once one has, the rest are only drawn small
//! to learn which others are incomplete, and the job ends in `RENDER_INCOMPLETE {pages}`.
use super::raster_pixels::render;
use super::raster_plan::{PageSpec, RasterPlan, incomplete};
use super::raster_write::{Encoded, encode, prepared};
use super::Prepared;
use crate::document::{Binding, Committed};
use crate::{Document, RenderOptions};
use foliark_model::{ErrorCode, ExportPolicy, Limits, PdfError};
use std::sync::atomic::Ordering;

/// The longest side, in px, of the small draw that only checks completeness.
const PROBE_SIDE: f64 = 32.0;

pub struct Job {
    plan: RasterPlan,
    edits: usize,
    next: usize,
    done: Vec<(PageSpec, Encoded)>,
    /// Encoded bytes so far (the final file adds a few hundred bytes per page).
    bytes: u64,
    bad: Vec<u32>,
}

impl Job {
    /// Judge eligibility on `doc` (the working copy at `binding`); draws nothing.
    pub fn new(
        doc: &Document,
        committed: &Committed,
        binding: Binding,
        policy: &ExportPolicy,
        limits: &Limits,
    ) -> Result<Self, PdfError> {
        Ok(Self {
            plan: RasterPlan::build(doc, committed, binding, policy, limits)?,
            edits: committed.edits.len(),
            next: 0,
            done: Vec::new(),
            bytes: 0,
            bad: Vec::new(),
        })
    }

    /// `(pages done, pages)`.
    pub fn progress(&self) -> (u32, u32) {
        (self.next as u32, self.plan.pages.len() as u32)
    }

    /// Advance by about `budget` work units (1,000 pixels each, at least one page). `Some` when
    /// the file is complete.
    pub fn step(
        &mut self,
        doc: &Document,
        limits: &Limits,
        budget: u64,
    ) -> Result<Option<Prepared>, PdfError> {
        let cancel = doc.cancel_handle();
        let mut spent = 0u64;
        while self.next < self.plan.pages.len() && spent < budget {
            if cancel.load(Ordering::Relaxed) {
                return Err(PdfError::new(ErrorCode::Cancelled));
            }
            let spec = self.plan.pages[self.next].clone();
            if self.bad.is_empty() {
                let px = render(doc, &self.plan, self.next, &spec)?;
                if px.complete {
                    let img = encode(&px)?;
                    drop(px);
                    self.bytes += img.flate.len() as u64;
                    let max = u64::from(limits.max_output_bytes);
                    if self.bytes > max {
                        return Err(PdfError::limit("maxOutputBytes", self.bytes, max));
                    }
                    self.done.push((spec.clone(), img));
                } else {
                    self.bad.push(self.next as u32);
                }
            } else {
                let probe = RenderOptions {
                    annotations: self.plan.annotations,
                    ..RenderOptions::at(PROBE_SIDE / spec.width_pt.max(spec.height_pt))
                };
                if !doc.render_page(self.next, probe)?.header.complete {
                    self.bad.push(self.next as u32);
                }
            }
            spent += u64::from(spec.width_px) * u64::from(spec.height_px) / 1000 + 1;
            self.next += 1;
        }
        if self.next < self.plan.pages.len() {
            return Ok(None);
        }
        if !self.bad.is_empty() {
            return Err(incomplete(&self.bad));
        }
        prepared(&self.done, &self.plan, self.edits, limits).map(Some)
    }
}

/// The whole job in one call (what the synchronous `export::prepare` runs).
pub fn run(
    doc: &Document,
    committed: &Committed,
    binding: Binding,
    policy: &ExportPolicy,
    limits: &Limits,
) -> Result<Prepared, PdfError> {
    let mut job = Job::new(doc, committed, binding, policy, limits)?;
    loop {
        if let Some(out) = job.step(doc, limits, u64::MAX)? {
            return Ok(out);
        }
    }
}
