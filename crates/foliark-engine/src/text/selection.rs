//! Hit testing and selection over clusters (spec §3.5).
use super::TextLayout;
use super::extract::{ClusterId, bounds};
use foliark_model::{Cluster, ErrorCode, Hit, PdfError, Point, Quad, Rect, TextRange};

fn inside(q: &Quad, p: Point) -> bool {
    let sides = [0, 1, 2, 3].map(|i| {
        let (a, b) = (q.0[i], q.0[(i + 1) % 4]);
        (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
    });
    sides.iter().all(|s| *s >= 0.0) || sides.iter().all(|s| *s <= 0.0)
}

/// 0 inside the quad, else the distance to its outline.
fn distance(q: &Quad, p: Point) -> f64 {
    if inside(q, p) {
        return 0.0;
    }
    (0..4)
        .map(|i| {
            let (a, b) = (q.0[i], q.0[(i + 1) % 4]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 {
                (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
        })
        .fold(f64::INFINITY, f64::min)
}

/// One quad around `quads`, oriented like the first (a rotated line stays a rotated box).
fn union_quad(quads: &[&Quad]) -> Quad {
    let first = quads[0];
    let o = first.0[0];
    let axis = quads
        .iter()
        .map(|q| Point {
            x: q.0[1].x - q.0[0].x,
            y: q.0[1].y - q.0[0].y,
        })
        .find(|v| v.x.hypot(v.y) > 1e-9)
        .unwrap_or(Point { x: 1.0, y: 0.0 });
    let len = axis.x.hypot(axis.y);
    let u = Point {
        x: axis.x / len,
        y: axis.y / len,
    };
    let mut v = Point { x: -u.y, y: u.x };
    let down = (first.0[3].x - o.x) * v.x + (first.0[3].y - o.y) * v.y;
    if down < 0.0 {
        v = Point { x: -v.x, y: -v.y };
    }
    let (mut u0, mut u1, mut v0, mut v1) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for p in quads.iter().flat_map(|q| q.0) {
        let (du, dv) = (
            (p.x - o.x) * u.x + (p.y - o.y) * u.y,
            (p.x - o.x) * v.x + (p.y - o.y) * v.y,
        );
        (u0, u1, v0, v1) = (u0.min(du), u1.max(du), v0.min(dv), v1.max(dv));
    }
    let at = |a: f64, b: f64| Point {
        x: o.x + a * u.x + b * v.x,
        y: o.y + a * u.y + b * v.y,
    };
    Quad([at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)])
}

/// Cluster indices per visual line id, in text order.
type Lines = Vec<(u32, Vec<usize>)>;

impl TextLayout {
    fn clusters(&self) -> &[Cluster] {
        self.page.clusters.as_deref().unwrap_or_default()
    }

    /// Index and distance of the cluster nearest to `p`; the earlier one on a tie.
    fn nearest(&self, p: Point) -> Option<(usize, f64)> {
        self.clusters()
            .iter()
            .enumerate()
            .map(|(i, c)| (i, distance(&c.quad, p)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The cluster whose quad contains `p`, else the nearest within `tolerance`.
    pub fn hit_test(&self, p: Point, tolerance: f64) -> Option<Hit> {
        let (i, _) = self.nearest(p).filter(|(_, d)| *d <= tolerance)?;
        let c = &self.clusters()[i];
        Some(Hit {
            start: c.start,
            end: c.end,
            word_index: self.meta[i].word,
            quad: c.quad,
        })
    }

    /// Both ends snap to the nearest cluster; the range runs from the earlier start to the
    /// later end. `None` on a page without text.
    pub fn select(&self, from: Point, to: Point) -> Option<TextRange> {
        let (a, b) = (self.nearest(from)?.0, self.nearest(to)?.0);
        let (c, d) = (&self.clusters()[a.min(b)], &self.clusters()[a.max(b)]);
        self.quads_for(c.start, d.end).ok()
    }

    /// The clusters a UTF-16 range `[start, end)` touches, grouped per visual line in text
    /// order, and whether it cuts into one. A cut cluster counts whole.
    fn touched(&self, start: u32, end: u32) -> Result<(Lines, bool), PdfError> {
        if start > end || end as usize > self.page.text.encode_utf16().count() {
            return Err(PdfError::new(ErrorCode::InvalidArgument));
        }
        let mut lines = Lines::new();
        let mut partial = false;
        for (i, (c, m)) in self.clusters().iter().zip(&self.meta).enumerate() {
            if c.start >= end || c.end <= start {
                continue;
            }
            partial |= c.start < start || c.end > end;
            match lines.iter_mut().find(|(l, _)| *l == m.line) {
                Some((_, idx)) => idx.push(i),
                None => lines.push((m.line, vec![i])),
            }
        }
        Ok((lines, partial))
    }

    /// One quad per visual line for the UTF-16 range `[start, end)` of `text`. A cluster the
    /// range cuts into counts whole and sets `partial_cluster`.
    pub fn quads_for(&self, start: u32, end: u32) -> Result<TextRange, PdfError> {
        let (lines, partial_cluster) = self.touched(start, end)?;
        let units: Vec<u16> = self.page.text.encode_utf16().collect();
        let clusters = self.clusters();
        Ok(TextRange {
            start,
            end,
            text: String::from_utf16_lossy(&units[start as usize..end as usize]),
            quads: lines
                .iter()
                .map(|(_, idx)| {
                    union_quad(&idx.iter().map(|&i| &clusters[i].quad).collect::<Vec<_>>())
                })
                .collect(),
            partial_cluster,
        })
    }

    /// What redacting the UTF-16 range `[start, end)` takes out (spec §4): one display-space
    /// bounding box per visual line and the id of every cluster the range touches.
    pub fn redaction_target(
        &self,
        start: u32,
        end: u32,
    ) -> Result<(Vec<Rect>, Vec<ClusterId>), PdfError> {
        let (lines, _) = self.touched(start, end)?;
        let clusters = self.clusters();
        let rects = lines
            .iter()
            .map(|(_, idx)| {
                let b = bounds(idx.iter().map(|&i| clusters[i].quad));
                Rect {
                    x: b[0],
                    y: b[1],
                    width: b[2] - b[0],
                    height: b[3] - b[1],
                }
            })
            .collect();
        let ids = lines
            .iter()
            .flat_map(|(_, idx)| idx.iter().map(|&i| self.meta[i].id.clone()))
            .collect();
        Ok((rects, ids))
    }
}
