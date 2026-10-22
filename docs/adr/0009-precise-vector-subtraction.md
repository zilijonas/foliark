# ADR 0009: Precise bounded vector subtraction for redaction (NEW-85)

Status: Accepted, 2026-10-22. Fills and clip outlines implemented by NEW-86 (see Implementation notes); strokes follow in NEW-87. Amends spec 5.4 (frozen: a change needs an ADR), 12, 16 and 17.3. Implemented by NEW-86 (fills) then NEW-87 (strokes).

## Context
Spec 5.4 (NEW-36/37) cuts only a lone axis-aligned fill rectangle exactly. Every other crossing path (curves, circles, rings, rotated shapes, even-odd stars, any stroke) is removed whole and disclosed as `VECTOR_COLLATERAL`. Owner requirement: precise cutting with no whole-shape collateral deletion; protected geometry inside a region unrecoverable from the output; public geometry outside preserved within a stated tolerance; bounded work; explicit refusal where unproven; pure Rust, permissive licence, wasm32, no C/C++.

Facts read from the code (not assumed):
- `PathItem` (interpret/mod.rs) holds `geometry` already in page user space (CTM applied), `fill: Option<FillRule>`, `stroke: Option<StrokeProps>` (width, cap, join, miter, dash array/offset, all in CTM-local units), `stroke_scale`, `clip_only`. The local path and the CTM are not kept. `vector.rs` recovers the stream-space affine by solving a rectangle's corners (`Back`), which only works for rectangles.
- The planner classifies per subpath by bounds. Fill rules act on the whole painted path (even-odd holes and nonzero winding span subpaths), so a precise cut must treat the painted path as one unit.
- hayro 0.8 strokes the local path with `kurbo::Stroke { width, join, miter_limit, start_cap == end_cap, dash_pattern, dash_offset }` under the CTM (hayro/src/path.rs), so strokes are expanded in local space and then transformed. Its only deviation is a renderer-side minimum device width (1 px, or `0.01/scale` for width 0).
- Work is charged through `limits::Budget` (`maxWorkUnits` default 5e9), far too coarse to bound one boolean call. Release profile is `panic = "abort"`: a panic in wasm is a worker reset, so inputs must be validated before the call, not trusted to be rejected by it.

## Candidates (sources: crates.io API, crate sources fetched 2026-10-22)
| Candidate | Licence | Latest | wasm / no_std / threads | Verdict |
|---|---|---|---|---|
| `i_overlay` 9.1.0 | MIT OR Apache-2.0 | 2026-10-14, ~10.3M downloads, rust 1.88 | `#![no_std]` + alloc, rayon only behind opt-in `allow_multithreading`, default features empty | Chosen. Difference/union/simplify with even-odd, non-zero, positive, negative; integer core (i16/i32/i64) with float adapters and fixed-scale API returning `Result`; documents coordinate limits; also has stroke/outline offset (not used, see below). Powers `geo` booleans. |
| `i_float` 5.0.0, `i_shape` 5.0.0, `i_tree` 0.20.0, `i_key_sort` 0.12.0 | MIT each | 2026-09/10 | no_std; `i_float` needs `libm` 0.2 | Transitive, MIT, compatible with AGPL-3.0-only |
| `geo` 0.33.1 | MIT OR Apache-2.0 | 2026-04-20 | std, large geo-types tree | Rejected: wraps i_overlay, adds weight and nothing we need |
| `linesweeper` 0.5.0 | MIT OR Apache-2.0 | 2026-10-05, ~93k downloads | std (`std::error`), polycool, arrayvec, smallvec | Rejected for now: self-described "early beta", source TODO on error bounds even for polylines. Curve-native and on kurbo 0.13 like our tree: revisit behind the same interface (Later). |
| `flo_curves` 0.8.1 | Apache-2.0 only | 2026-08-25, ~371k | std; ouroboros, roots, itertools | Rejected: float ray-casting booleans without a robustness bound, heavier graph |
| `clipper2` 0.6.0 | MIT OR Apache-2.0 | 2026-05 | depends on `clipper2c-sys` (C++) | Forbidden: C/C++ |
| `geo-clipper` 0.9.0 | ISC | 2025-02 | C++ Clipper binding | Forbidden: C/C++ |
| `clipper2-rust` 1.2.0 | BSL-1.0 | 2026-09-18 | pure Rust port | Rejected: young port, BSL not our usual notice set, no advantage over i_overlay |
| `lyon` | MIT OR Apache-2.0 | 1.0.x | - | Not a boolean library (tessellation, path types) |
| `kurbo` 0.13.1 | Apache-2.0 OR MIT | 2026-05-13 | already in the graph (hayro re-exports it) | Used for flatten, stroke expansion and dashing. No boolean ops. |

Stroke expansion: kurbo, not i_overlay's offset. kurbo is what the renderer uses with the same `Stroke` fields, so the cut geometry equals what hayro draws (curve-native offsetting, PDF miter-limit semantics, dashing built in). i_overlay's stroke has no dashes and uses integer rounding; its integer miter clips sharper than 5 degrees.

Wasm size (scratch crate in the session scratchpad, not in the repo; `cargo build --release --target wasm32-unknown-unknown`, workspace release profile `opt-level = "s"`, lto, `panic = "abort"`, strip; kurbo 0.13.1 `libm`): kurbo stroke + dash + flatten alone 77,688 B raw / 34,599 B gzip -9; the same plus i_overlay 9.1.0 (`simplify_shape` + fixed-scale `Difference`) 247,055 B raw / 90,596 B gzip -9. Marginal cost about +169 KB raw / +56 KB gzip, against the shipped 5.36 MB raw / 2.05 MB gzip (docs/performance/comparison.md): about 3 percent. No wasm-opt run; the shipped build's own number will differ slightly (shared code, `opt-level = 3` kurbo).

## Decision
Dependency: `i_overlay = { version = "=9.1.0", default-features = false }` in the workspace and `foliark-engine`. No features. Lockfile adds i_overlay, i_float, i_shape, i_tree, i_key_sort, libm (already present via other crates, verify). All MIT or MIT OR Apache-2.0; NEW-86 adds them to the existing third-party notices check. Never enable `allow_multithreading` (rayon needs threads and `SharedArrayBuffer` in wasm).

### Algorithm
Everything runs in display space (spec 2), where regions are axis-aligned rectangles. One painted path (all subpaths between the first construction operator and the paint operator) is one unit.

1. Prefilter. If the path's display bounds, grown by the existing conservative reach (half width x max(1, miter, cap corner) along the CTM's widest axis), miss every region: untouched, byte-exact, nothing re-emitted. The bound is only a prefilter, never a verdict.
2. Region set. Per page, union the region rectangles inflated by `GUARD` (below) with i_overlay `Union` (rects only, so fill rule is irrelevant). A path is clipped by only the unioned regions its grown bounds reach.
3. Ink. Fill: the path's Beziers mapped to display space by affine (exact), flattened with `kurbo::flatten` at `FLATNESS_PT`. Stroke: stroke the local path (`ctm.inverse() * geometry`) with the item's `StrokeProps` as hayro builds them (width floored at `0.01/widest_axis`, as hayro does for width 0), `kurbo::stroke` with tolerance `FLATNESS_PT / widest_axis`, dashes via the same call, then map the outline by CTM and display and flatten. The outline is filled with non-zero. Non-uniform scale is exact because expansion happens before the CTM. Fill and stroke of one `B`/`b` operator are two inks painted fill first, then stroke.
4. Resolve and subtract. Fill ink: `simplify_shape(rule)` with the path's own rule (even-odd or non-zero) turns winding into disjoint shapes, then `overlay_with_fixed_scale(regions, Difference, NonZero, SCALE)` (regions are disjoint after step 2, so the single fill rule of the call cannot disagree with the path's). Stroke ink: `simplify_shape(NonZero)` then the same difference.
5. Classes. Ink disjoint from the inflated regions: untouched. Empty difference: removed (path operators dropped; a `W`/`W*` clip is kept verbatim followed by `n`, as today). Otherwise cut.
6. Emission, replacing the original operator span: every result contour as `m l ... h` mapped back to stream space by `(user_to_display * ctm).inverse()`, painted `f*` for fill ink (orientation-independent; the shapes are disjoint so parity equals the original rule). Stroke ink is painted `f*` too (see Fidelity). A `W` clip path is emitted first as today. Clip rule of the original path is untouched.
7. Singular CTM (determinant under `1e-12 * widest_axis^2`) or non-finite matrix: the painted ink has zero area (a point or segment, hairline for strokes). If its flattened polyline comes within `GUARD` of a region, the whole path is removed (nothing visible is lost, nothing recoverable stays); otherwise untouched. Reported as removed, not collateral.
8. Clips are not concealment. The ink tested is the full path geometry, whether or not a clip would hide it, and whatever a later `Q` restores. Nothing is ever kept hidden under a clip. Non-painting clip paths are still kept (accepted: they are the visible frame, not ink); a painted `W` path that is cut keeps its clip shape verbatim, which still contains the original outline (Later: subtract regions from clip shapes too).
9. Patterns and shadings: the cut path keeps its `scn`/`sh` state, so the pattern cell still tiles through the new shape (pattern space is independent of the CTM). Tiling cells containing text or images stay `REDACT_PATTERN_UNSUPPORTED`; shading `sh` stays (procedural, covered by the fill) unchanged.
10. Shared Forms and streams: unchanged; NEW-38 clones the occurrence before the rewrite, the original occurrences keep their bytes.

### Stroke emitted as fill
A cut stroke becomes a filled outline, so it must paint in the stroking colour with fill operators. The scan (vector.rs) records, per `q` level, the raw stroking colour operators (`G RG K`, or `CS` plus `SC`/`SCN`) seen in the same stream and mirrors them as `g rg k` / `cs sc scn` inside `q ... Q` around the emitted path. Refusals instead of guessing (all `REDACT_VECTOR_UNSUPPORTED {feature, pages}` before the writer is touched, no bytes):
- `strokeColorInherited`: no stroking colour operator in force within the stream (a Form that inherits the page's colour).
- `strokeAlphaDiffers`: stroke alpha (`CA`) differs from fill alpha (`ca`) in force; `strokeBothAlpha`: a cut `B` with any alpha under 1 (PDF paints fill+stroke of one operator as a knockout group).
`PathItem` gains `stroke_alpha`, `fill_alpha` (NEW-87 confirms DrawProps exposes them; if not, an additive P2-style vendor patch, README line). Uncut strokes are never rewritten, so none of this applies to them.

### Interfaces
New `crates/foliark-engine/src/redaction/geometry.rs` (`pub(super)`, owned by NEW-86, extended by NEW-87). Spec 14 lists no separate module; it belongs to `::redact`.
```rust
pub(super) const FLATNESS_PT: f64 = 0.01;          // curve flattening, display space
pub(super) const GUARD_PT: f64 = 0.01;             // regions are inflated by this before subtraction
pub(super) const SCALE: f64 = 1024.0;              // fixed-point grid 1/1024 pt
pub(super) const MAX_COORD_PT: f64 = 131_072.0;    // |display coordinate| for any path that reaches a region
pub(super) const MAX_VERTICES: usize = 250_000;    // flattened ink vertices per painted path
pub(super) const MAX_DASHES: usize = 100_000;      // dash segments per painted path (estimated before kurbo runs)

pub(super) type Contour = Vec<[f64; 2]>;           // display space, implicitly closed
pub(super) struct RegionSet { /* disjoint inflated union, display space */ }
pub(super) enum Cut { Untouched, Removed, Pieces(Vec<Contour>) }

pub(super) fn region_set(rects: &[[f64; 4]], budget: &mut Budget) -> Result<RegionSet, PdfError>;
/// Fill ink of one painted path, already in display space. `Err(LimitExceeded{limit:"vectorVertices"|"vectorCoordinate"})` over a cap.
pub(super) fn cut_fill(path_display: &BezPath, rule: FillRule, regions: &RegionSet, budget: &mut Budget) -> Result<Cut, PdfError>;
/// NEW-87. `local` is the stream-space path, `to_display = user_to_display * ctm`.
pub(super) fn cut_stroke(local: &BezPath, props: &StrokeProps, to_display: Affine, regions: &RegionSet, budget: &mut Budget) -> Result<Cut, PdfError>;
/// Display contours back to stream-space `m l h` text; `Err(Internal)` if `to_display` is singular or a number is not finite.
pub(super) fn emit(contours: &[Contour], to_display: Affine) -> Result<Vec<u8>, PdfError>;
```
Every `Err` is raised in the planning pass, before the writer is touched, exactly as today. Callers in `vector.rs`: the `Fate::Pieces` arm becomes `Fate::Cut(Vec<u8>)` (emitted bytes); `Back`, `minus`, `corners`, `RectCut` and the per-subpath fates are deleted, fates are per painted path. `PathItem` gains `ctm: Matrix` (local to user) so stream space is recovered exactly instead of from rectangle corners. `plan.rs::plan_vectors` keeps only the prefilter and returns one `PlannedVector` per painted path; exact classification moves to `geometry` (it needs the flattened ink).

### Bounds and refusals
- Validation precedes every i_overlay call: all coordinates finite and `|x|,|y| <= MAX_COORD_PT` (with `SCALE` the integer coordinates stay under 2^27 on i32, inside i_overlay's documented range), scale positive and finite. Violation: `LIMIT_EXCEEDED {limit: "vectorCoordinate"}`. i_overlay's infallible APIs panic on bad bounds, so only fixed-scale `Result` APIs and a pre-validated input are used.
- `MAX_VERTICES` checked while flattening (abort at the cap, not after), after stroke expansion too. Dashes: count estimated from flattened length and pattern length before kurbo is called; over `MAX_DASHES` -> `LIMIT_EXCEEDED {limit: "vectorDashes"}`.
- Work charge, before each boolean: `(n + m) * (floor(log2(n + m)) + 1)` units for `n` ink and `m` region vertices; 1 unit per flattened segment and per kurbo output element. Over `maxWorkUnits` -> `LIMIT_EXCEEDED {limit: "maxWorkUnits"}`. The vertex caps, not the budget, bound one call's runtime (a call is not interruptible); NEW-86 records the measured time at the cap on the bench machine and lowers `MAX_VERTICES` if one call exceeds a stepping slice, never raises the budget.
- Typed refusals added: `REDACT_VECTOR_UNSUPPORTED {feature, pages}` (strokeColorInherited, strokeAlphaDiffers, strokeBothAlpha). Nothing falls back silently to collateral removal.

### Tolerances (declared, each asserted by a test)
- Privacy: no output ink inside a region. Inflation by `GUARD_PT` (0.01) plus grid rounding (<= 0.0005 pt per vertex) keeps every emitted vertex outside the region; oracle epsilon 0.005 pt (above f32 quantisation 0.001 at page scale, below the guard).
- Public geometry: output ink outside regions grown by `GUARD_PT` differs from input ink by at most `FLATNESS_PT + 1/SCALE` (about 0.011 pt) Hausdorff, curve-flattened only on cut paths. Everything not reached by a region is byte-exact.
- Disclosed fidelity changes: (a) a cut path's curves become 0.01 pt polylines; (b) a cut stroke becomes a fill, so dash/cap/join structure is gone from the file (shape identical within tolerance) and viewers that widen thin strokes to a minimum device width (hayro: 1 px) draw a cut hairline at its nominal width; (c) up to 0.01 pt of public ink next to a region edge is also removed. Reported in `redactions[].removed` as `vectorPathsCut` (replaces `vectorRectsCut`; NEW-86 updates DTO, ts-rs output, SDK types and spec 1 in one commit) and loss `VECTOR_CUT_APPROXIMATED`; `VECTOR_COLLATERAL` is no longer emitted. Singular-CTM removals count as `vectorSubpathsRemoved`.

## Independent verification
Rust tests prove behaviour through `redact_vectors` as in `tests/redact_fills.rs`; oracles never import foliark.
1. Decoded geometry (`tests/oracles/objects.py` extended, or new `vector_cut.py`): PyMuPDF `get_drawings()` on the output; every fill polygon and stroke segment of the output must stay outside each region minus 0.005 pt (none inside, no segment crossing), and for fill-ink fixtures the output polygon vertex count must be non-zero (non-vacuous: input must have drawings inside the region, as the existing check does).
2. Clip-stripped, unclipped render: `pikepdf.parse_content_stream` over the page and every Form stream of the output, drop `W`/`W*`, unparse, render with PyMuPDF at 4x with `fill: null` redactions; ink inside every region must be zero. This catches concealment by clip.
3. Fidelity outside regions: `tests/oracles/fidelity.py` pixel diff (existing masks, margin 0.5 pt) input vs output; and a polygon check (decoded geometry of input minus inflated regions, sampled at a 0.05 pt grid, versus output) with hit/miss bands of `FLATNESS_PT + 1/SCALE` + 0.001.
4. Property test in the crate (no i_overlay in the oracle): random simple and self-intersecting polygons and rect sets from a fixed-seed LCG; a sample point is in the output iff it was in the input under the path's rule and outside every inflated region, outside a 0.012 pt band around edges. Own winding/even-odd point test.
5. Mutation controls (each must fail its fixture): drop `GUARD_PT`; ignore the fill rule; stroke in display space instead of local; skip the dash cap; keep `W` and paint nothing.
6. In-engine verify (spec 5.7) re-reads the written page model (not the plan): no path ink, fill polygon or stroke outline, within 0.005 pt of a region, tested with segment-vs-rect (not i_overlay). Failure -> `REDACT_VERIFY_FAILED`.

## Fixture matrix
Fills (NEW-86): `rect-partial-overlap` (rect cut to L shape, neighbours byte-kept), `circle-crossing` (4 Beziers), `ring-with-hole` (nonzero opposite winding, region over the hole only and over the ring), `even-odd-star` (self-intersecting, even-odd centre hole kept outside), `nonzero-star` (centre filled), `rotated-path` (page /Rotate 90 plus rotated CTM), `flipped-nonuniform-ctm`, `nested-clip` (path crossing region under `W n`, clip stays, geometry cut), `clipped-away-geometry` (path wholly hidden by a clip yet inside a region: removed), `outlined-text-glyph` (path glyphs, crossing glyph cut, not removed), `multi-region-union` (overlapping regions, even-odd path), `region-fully-covers` (removed, `W n` kept), `pattern-fill-cut` (tiling path cell cut keeps pattern), `singular-ctm`, `huge-path-near-budget` (just under `MAX_VERTICES`: succeeds; just over: `LIMIT_EXCEEDED vectorVertices`), `coordinate-over-cap` (`vectorCoordinate`), `far-path-untouched` (byte-identical stream).
Strokes (NEW-87): `thick-mitered-stroke` (30 degree spike, tip in region), `round-caps` (cap inside region only), `square-cap-corner`, `dashed-curve` (dash gap across the edge), `nonuniform-scaled-stroke` (x10, miter limit 1), `fill-and-stroke` (`B`, alpha 1), `fill-and-stroke-alpha` (refused), `stroke-color-inherited` (Form, refused), `stroke-alpha-differs` (refused), `hairline-stroke` (width 0), `stroke-pattern-colour` (`/P1 SCN` mirrored), `dash-explosion` (`vectorDashes`), `closed-subpath-s` (`s`/`b` closure kept), `table-rules` (many 0.5 pt rules, one region).
Each case asserts: report counts, stream bytes outside the span unchanged, oracles 1 to 3, mutation control where listed.

## Ownership (sequential, no overlap)
- NEW-86 (first): Cargo.toml and Cargo.lock (dependency), `redaction/geometry.rs` (helper, fills), `redaction/mod.rs`, `redaction/vector.rs` (path-level fates, fills; stroked items keep today's behaviour until NEW-87), `redaction/plan.rs` (prefilter, per-path unit), `interpret/mod.rs` (`PathItem.ctm`), spec 5.4/12/16/17.3 and 1 DTO edits, notices, `tests/redact_fills.rs`, `tests/vector_support`, `tests/oracles/vector_cut.py`, in-engine verify in `export/mod.rs`, property test.
- NEW-87 (after NEW-86 merges): `geometry.rs` (`cut_stroke`), `vector.rs` (stroke arm, colour mirroring in `scan`), `interpret/mod.rs` (alpha fields), `tests/redact_strokes.rs`, oracle additions, `REDACT_VECTOR_UNSUPPORTED` wiring. No other agent edits these files meanwhile.

## Commands
`~/ai/bin/memory-queue run --owner <id> --label 'foliark <what>' --need-gb 4 --restartable --wait 1800 -- <cmd>` for each: `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p foliark-engine --test redact_fills --test redact_strokes --test redaction_plan`; `cargo test --workspace --no-fail-fast`; `cargo build --release --target wasm32-unknown-unknown -p foliark-wasm` (size delta recorded); oracles `tests/oracles/.venv/bin/python -I tests/oracles/vector_cut.py <out.pdf> --spec <spec.json>` and `.../fidelity.py`; `cargo tree -p foliark-engine -e normal | grep -E 'i_|libm'` (no rayon, no C).

## Spec amendments (applied by NEW-86)
Spec 5.4 replacement text:
> 4. **Vectors**, per painted path (all its subpaths together, under the path's fill rule): the ink (fill area; stroke outline built from the stroke's own width, caps, joins, miter limit and dashes in the path's local space) is subtracted from the union of the regions, display space, regions inflated by 0.01 pt. (a) Ink entirely inside -> the path is removed. (b) Ink crossing -> the path is replaced by the difference, curves flattened to 0.01 pt, painted with `f*`; a stroke becomes a filled outline in the stroking colour (`vectorPathsCut`, loss `VECTOR_CUT_APPROXIMATED`). (c) Ink outside -> untouched, byte-exact. Geometry is tested whether or not a clip hides it. A path with all ink removed keeps its W/W* clip and paints `n`. Non-painting clip paths are kept. `sh` is kept (procedural, covered by the fill). A zero-area path (singular CTM) near a region is removed. Tiling-pattern paint under a region whose cell contains text or images -> `REDACT_PATTERN_UNSUPPORTED`. Over a work cap -> `LIMIT_EXCEEDED {vectorVertices | vectorDashes | vectorCoordinate}`. A cut stroke whose colour, stroke/fill alpha or `B` alpha cannot be reproduced as a fill -> `REDACT_VECTOR_UNSUPPORTED {feature}`. No path is removed whole merely for crossing a region.

Spec 16: delete "exact curve/stroke boolean subtraction (winding, dashes, caps, joins)"; add "curve-preserving cuts (keep Beziers outside the cut), region subtraction from clip shapes, inherited stroke colour via resolved colour". Spec 12: add `REDACT_VECTOR_UNSUPPORTED {feature, pages}`, `LIMIT_EXCEEDED` limit names above. Spec 17.3: resolved, collateral no longer exists.

## Consequences and limitations
- A cut path loses its Beziers (0.01 pt polyline) and a cut stroke its stroke structure; both disclosed and measured. Untouched geometry never changes.
- A painted `W` path that is cut still carries its original outline as a clip (accepted today; listed in Later).
- Refusals are by design for inherited stroke colour and unequal stroke/fill alpha; raster export stays the escape hatch.
- Not measured: cut quality on real documents, boolean time at the vertex cap (NEW-86 measures), shipped-build wasm size (scratch estimate above).
- Upgrade path: `linesweeper` once it leaves beta, behind the same `geometry` interface, would keep curves.

## Implementation notes (NEW-86, fills and clips)
Deviations from the design above, each tested in `tests/redact_fills.rs`:
- Regions are not pre-unioned: they go to `i_overlay` as one clip set under non-zero (all rectangles wind alike, so overlaps stay in); an even-odd path is resolved with `simplify_shape(EvenOdd)` first. Only regions whose bounds touch the group are passed.
- Subpaths are grouped by touching display bounds (sweep plus union-find). A group's winding cannot depend on another group, so only groups whose ink really meets a region are replaced; the rest keep their operators byte for byte. A cut group is written under the path's own paint operator (disjoint contours, holes wind against their outer, valid under either fill rule), not `f*`.
- A group is "touched" when an edge meets a grown region or a region lies inside the ink (judged on the flattened polygons, independent of `i_overlay`); otherwise nothing is rewritten.
- The stream-to-display affine is fitted from the stream's own vertices against the model's (best conditioned triple, verified on every pair) instead of adding `PathItem.ctm`; a path whose ink meets a region while the fit is impossible is `INTERNAL`.
- Clips (lead amendment): the outline of every `W`/`W*` path (painted or `W n`) is cut like ink, but only when its outline meets a region (a clip that merely encloses a region holds none of it). A clip wholly inside the regions becomes `0 0 0 0 re W n`. A painted path's clip is written after its paint. BBox clips of Forms and patterns have no `n` in the stream and are not planned.
- The plan only knows which paths may reach a region: `vectorPathsCut` in `plan.report` is a bound, the export report counts what the stages did; `check_stages` requires every planned entry to be accounted for as removed, cut or untouched.
- `REDACT_VECTOR_UNSUPPORTED` is not used by fills (nothing to refuse beyond the caps); NEW-87 introduces it.
- Measured: one saw polygon of 249,000 vertices crossing one region, whole `redact_vectors` call, native release build on the Mac mini: 717 ms.
