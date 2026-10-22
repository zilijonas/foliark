//! Page search over `text` with a reversible normalization map (spec §3.5).
use super::TextLayout;
use foliark_model::{SearchHit, SearchOptions};
use unicode_normalization::char::{decompose_canonical, is_combining_mark};

/// Normalized text and, per unit, the UTF-16 span of the source code point it came from plus
/// whether it is that code point's first and last unit.
struct Normalized {
    chars: Vec<char>,
    source: Vec<(u32, u32, bool, bool)>,
}

/// N1 to N4 for one code point.
fn units(c: char, opts: &SearchOptions) -> Vec<char> {
    let mut lower = c;
    if !opts.case_sensitive {
        let mut it = c.to_lowercase();
        if let (Some(l), None) = (it.next(), it.next()) {
            lower = l;
        }
    }
    let mut out = Vec::new();
    let mut push = |u: char| {
        out.push(match u {
            '\u{2018}'..='\u{201B}' => '\'',
            '\u{201C}'..='\u{201E}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            u if u.is_whitespace() => ' ',
            u => u,
        });
    };
    let expanded = match lower {
        '\u{FB00}' => "ff",
        '\u{FB01}' => "fi",
        '\u{FB02}' => "fl",
        '\u{FB03}' => "ffi",
        '\u{FB04}' => "ffl",
        '\u{FB05}' | '\u{FB06}' => "st",
        _ => "",
    };
    for e in expanded.chars().chain(expanded.is_empty().then_some(lower)) {
        if opts.diacritic_sensitive {
            push(e);
        } else {
            decompose_canonical(e, |d| {
                if !is_combining_mark(d) {
                    push(d);
                }
            });
        }
    }
    out
}

fn normalize(text: &str, opts: &SearchOptions) -> Normalized {
    let mut n = Normalized {
        chars: Vec::new(),
        source: Vec::new(),
    };
    let mut at = 0_u32;
    for c in text.chars() {
        let end = at + c.len_utf16() as u32;
        let us = units(c, opts);
        let last = us.len().saturating_sub(1);
        for (i, u) in us.into_iter().enumerate() {
            n.chars.push(u);
            n.source.push((at, end, i == 0, i == last));
        }
        at = end;
    }
    n
}

impl TextLayout {
    /// Up to `room` non-overlapping matches of `query` on this page, in text order. Match ends
    /// snap to whole clusters; `partial_cluster` says the match covered only part of one.
    pub fn search(&self, query: &str, opts: &SearchOptions, room: usize) -> Vec<SearchHit> {
        let mut q = normalize(query, opts).chars;
        q.dedup_by(|a, b| *a == ' ' && *b == ' ');
        let q: Vec<char> = q.into_iter().collect::<String>().trim().chars().collect();
        let n = normalize(&self.page.text, opts);
        let clusters = self.page.clusters.as_deref().unwrap_or_default();
        let units: Vec<u16> = self.page.text.encode_utf16().collect();
        let mut hits = Vec::new();
        let mut i = 0;
        while !q.is_empty() && i + q.len() <= n.chars.len() && hits.len() < room {
            let j = i + q.len();
            let boundary = !opts.whole_word
                || (i == 0 || !n.chars[i - 1].is_alphanumeric())
                    && (j == n.chars.len() || !n.chars[j].is_alphanumeric());
            if n.chars[i..j] != q[..] || !boundary {
                i += 1;
                continue;
            }
            let (first, last) = (n.source[i], n.source[j - 1]);
            // Marks dropped by N3 right after the last matched unit belong to its base.
            let next = n.source.get(j).map_or(units.len() as u32, |u| u.0);
            let (s, e) = (first.0, last.1.max(next));
            let from = clusters.partition_point(|c| c.end <= s);
            let to = clusters.partition_point(|c| c.start < e);
            let (start, end) = (clusters[from].start, clusters[to - 1].end);
            let mut range = self
                .quads_for(start, end)
                .expect("cluster offsets are in range");
            range.text = String::from_utf16_lossy(&units[start as usize..end as usize]);
            range.partial_cluster = (start, end) != (s, e) || !first.2 || !last.3;
            hits.push(SearchHit {
                range,
                page_index: self.page.page_index,
                revision: self.page.revision,
            });
            i = j;
        }
        hits
    }
}
