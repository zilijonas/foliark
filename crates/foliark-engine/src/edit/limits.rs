//! The room around a paragraph, from the page's own evidence (ADR 0015, spec §6.2). A pure
//! function of display-space boxes, so the stable width, the extended width and push-down's
//! budget all read the same numbers.

type B = [f64; 4];

/// What the page shows around the selection. All boxes are display-space `[x0, y0, x1, y1]`.
pub(crate) struct Evidence<'a> {
    /// The selection's bounds.
    pub sel: B,
    pub last_baseline: f64,
    pub descent: f64,
    /// Space width of the paragraph's font.
    pub space: f64,
    /// Display width and height of the page.
    pub page: (f64, f64),
    /// Visible words outside the selection.
    pub words: &'a [B],
    /// Painted non-text items: paths, images, shadings.
    pub items: &'a [B],
    /// Every visible cluster of the page.
    pub text: &'a [B],
}

pub(crate) struct Limits {
    /// The nearest other word starting right of the selection on its lines, else infinity.
    pub nearest_word: f64,
    /// Spec §6.2's `wrapRight`: the stable width's right edge.
    pub wrap_right: f64,
    /// ADR 0015's `maxWrapRight`: the extended width's right edge, never left of the selection.
    pub max_wrap_right: f64,
    sel: B,
    last_baseline: f64,
    /// Words and non-container, non-decoration items.
    obstacles: Vec<B>,
    /// The bottom of the tightest container, else infinity.
    container_bottom: f64,
    /// `H` minus the page's smallest visible text y.
    text_bottom: f64,
    page_h: f64,
}

impl Limits {
    /// The top of the nearest obstacle below the last baseline that overlaps `[x0, right]`, else
    /// the page's text margin; capped by the tightest container and the page, never above the
    /// selection's bottom.
    pub fn bottom_limit(&self, right: f64) -> f64 {
        let [x0, _, _, y1] = self.sel;
        let below = self
            .obstacles
            .iter()
            .filter(|o| o[1] >= self.last_baseline && o[2] > x0 && o[0] < right)
            .map(|o| o[1])
            .fold(f64::INFINITY, f64::min);
        let base = if below.is_finite() {
            below
        } else {
            self.text_bottom
        };
        base.min(self.container_bottom).min(self.page_h).max(y1)
    }
}

fn contains(outer: B, inner: B) -> bool {
    const EPS: f64 = 0.01;
    outer[0] <= inner[0] + EPS
        && outer[1] <= inner[1] + EPS
        && outer[2] >= inner[2] - EPS
        && outer[3] >= inner[3] - EPS
}

pub(crate) fn limits(e: &Evidence) -> Limits {
    let [x0, top, x1, y1] = e.sel;
    let (page_w, page_h) = e.page;
    let beside = |w: &&B| w[0] > x1 && w[1] < y1 && w[3] > top;
    let nearest_word = e
        .words
        .iter()
        .filter(beside)
        .map(|w| w[0])
        .fold(f64::INFINITY, f64::min);
    let wrap_right = if nearest_word.is_finite() {
        x1
    } else {
        e.text.iter().map(|b| b[2]).fold(x1, f64::max)
    };

    let mut obstacles = e.words.to_vec();
    let (mut cap_x, mut cap_y) = (f64::INFINITY, f64::INFINITY);
    for &b in e.items {
        if contains(b, e.sel) {
            cap_x = cap_x.min(b[2]);
            cap_y = cap_y.min(b[3]);
        } else if !(b[2] > x0 && b[0] < x1 && b[1] < e.last_baseline + e.descent) {
            obstacles.push(b);
        }
    }
    let nearest = obstacles
        .iter()
        .filter(beside)
        .map(|o| o[0])
        .fold(f64::INFINITY, f64::min);
    let left_margin = e.text.iter().map(|b| b[0]).fold(f64::INFINITY, f64::min);
    let free = e
        .text
        .iter()
        .map(|b| b[2])
        .fold(page_w - left_margin, f64::max)
        .min(page_w);
    let max_wrap_right = x1.max(free.min(nearest - e.space).min(cap_x - e.space));
    let margin = e.text.iter().map(|b| b[1]).fold(f64::INFINITY, f64::min);
    Limits {
        nearest_word,
        wrap_right,
        max_wrap_right,
        sel: e.sel,
        last_baseline: e.last_baseline,
        obstacles,
        container_bottom: cap_y,
        text_bottom: page_h - margin,
        page_h,
    }
}
