/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Output checks (#93): plate edges, maximum height, empty layers and
//! floating parts.
//!
//! Reports [`CheckKind::PlateEdge`], [`CheckKind::TooTall`],
//! [`CheckKind::EmptyLayer`] and [`CheckKind::FloatingPart`] findings, all
//! read from the shared [`PlateRegions`] plus, for the plate edge and the
//! empty plate, the vector contours:
//!
//! - **Plate edge**, one finding per part. A part is a printed body (the
//!   solid regions joined through every overlap between layers) with its
//!   polygons that reach past the plate: those that light pixels of the
//!   body, and those that share a pixel of the printer's grid extended past
//!   the screen with them, on the same layer or straight above or below,
//!   whatever their leaf (the links bodies are joined by, so a part that
//!   leaves the screen further up or down, or whose supports meet it only
//!   past the plate, stays one part). A polygon too thin to light a pixel
//!   even there joins the polygons whose bounding box meets its own on its
//!   layer and the next ones: mesh slivers stay with their part. Bodies
//!   such polygons join are one part.
//!   - *Cut by the plate edge* (error): some polygon of the part reaches past
//!     the plate, so what lies beyond the screen is not printed.
//!   - *Touches the screen edge* (warning): the part lights the screen's
//!     outermost pixel row or column but nothing of it lies past the plate.
//!     Nothing is lost, but it has no margin left.
//!   - *Outside the printable area* (error): polygons past the plate that
//!     light no pixel and join no body (a part wholly off the plate, or a
//!     sliver too thin to light a pixel). The same links join them into
//!     parts, each reported once, over its whole layer range and naming its
//!     leaves.
//!
//!   A part's finding runs from the first to the last layer it touches the
//!   edge or reaches past it, focused on its edge pixel nearest to its
//!   region's centroid on the first layer, or on the farthest vertex past
//!   the plate when nothing of it is on the screen's edge there.
//! - **Too tall** (error). The top of the highest layer with solid is above
//!   the printer's maximum height. One finding, from the first layer with
//!   solid that ends above the limit to the highest one.
//! - **Empty layers**, consecutive ones grouped into one finding. Before the
//!   first solid layer (error: nothing is cured on the plate, so the first
//!   solid layer has nothing to stick to); between solid layers (error: a
//!   gap, so everything above it rests on nothing); after the last solid
//!   layer (info: they print nothing, and the printer file leaves them
//!   out, #309). The layer count stops at the last layer cut below the
//!   plate's top (#226), so a plate sliced now ends in one only when its
//!   top is too small to light a pixel; slices saved before can. The
//!   printer file gives an empty layer it keeps one placeholder pixel, so
//!   the printer does not stall on it (#309).
//! - **Nothing to print** (error). Contours on the plate, yet no layer has
//!   a single lit pixel: the printer file would print nothing.
//! - **Floating part** (error). A printed body whose lowest region is on
//!   layer [`PLATE_LAYERS`] (counting from 0) or higher: nothing below it in
//!   any earlier layer ever reaches the plate. A body that starts on one of
//!   the first [`PLATE_LAYERS`] layers rests on the plate, as in the island
//!   check: the raft and bottom layers absorb a body that starts a layer up.
//!   A body that starts right above an empty layer is left to the
//!   empty-layer finding, and one whose part is cut by the plate edge on
//!   the layer right below it to the plate-edge finding: each explains it.
//!   It suggests a support under the body's largest bottom region when that
//!   region has model in it. A body
//!   smaller than [`OutputOptions::speck_volume_mm3`] is a *floating speck*
//!   (warning): too small for a support to hold. It suggests erasing it
//!   (#156): its pixels, grown a pixel on every side, are erased on the next
//!   slice ([`crate::fill::erase_stack`]).
//!
//! Joining solid within a layer follows the shared regions, which are
//! 8-connected: a part that touches a body only by a corner pixel belongs to
//! it. How weakly a part is held is the island check's question, not this
//! one's.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::iter;
use std::ops::Range;
use std::sync::Arc;

use rayon::prelude::*;
use sc_sl1cer::contour::{ClosedRing, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::raster::pixel_map::PixelMap;

use crate::checks::thin;
use crate::fill::CavityFill;
use crate::finding::{
    CheckKind, Finding, MAX_FOOTPRINT_LAYERS, PlatePoint, RoleScope, Severity, SuggestedFix,
};
use crate::input::{ReviewInput, Role};
use crate::regions::{LayerPixels, PlateRegions, Region};
use crate::runner::ReviewContext;
use crate::runs::{self, Run, for_each_overlap};
use crate::union_find::{Groups, RegionRef, UnionFind};

/// The layers, counted from 0, on which a printed body rests on the plate:
/// one whose lowest layer is below this does not float. It must match the
/// island check's `PLATE_LAYERS`, so that both checks agree on what stands
/// on the plate.
pub const PLATE_LAYERS: usize = 2;

/// Settings of the output checks.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputOptions {
    /// How far, in millimetres, a contour may reach past the edge of the
    /// plate before it counts as outside the printable area. Only absorbs
    /// rounding in placement: anything past the edge is not printed.
    pub edge_tolerance_mm: f64,
    /// How far, in millimetres, the top of the print may rise above the
    /// printer's maximum height before it is too tall. Only absorbs
    /// rounding in the layer heights.
    pub height_tolerance_mm: f64,
    /// Floating bodies smaller than this, in cubic millimetres, are specks:
    /// a warning without a support suggestion. The default, 0.001 mm³, is a
    /// 0.1 mm cube: about the smallest feature a resin printer cures
    /// reliably, and smaller than any support tip.
    pub speck_volume_mm3: f64,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            edge_tolerance_mm: 1e-3,
            height_tolerance_mm: 1e-3,
            speck_volume_mm3: 1e-3,
        }
    }
}

/// Finds the output problems of the plate.
pub fn run(ctx: &ReviewContext<'_>, options: &OutputOptions) -> Vec<Finding> {
    run_with_erasures(ctx, options).0
}

/// Finds the output problems of the plate, with the pixels of every
/// floating speck: a [`SuggestedFix::Erase`] names its [`CavityFill`] by
/// index in the second list (#156).
pub fn run_with_erasures(
    ctx: &ReviewContext<'_>,
    options: &OutputOptions,
) -> (Vec<Finding>, Vec<CavityFill>) {
    let _clock = crate::clock::scope("output.total");
    let contours = scan_contours(ctx.input, options.edge_tolerance_mm);
    let bodies = ctx.regions.group_solid(|_, _| true);
    let edge = plate_edge(ctx, &bodies, &contours);
    let mut findings = edge.findings;
    let Some(extent) = SolidExtent::of(ctx.regions) else {
        findings.extend(nothing_to_print(ctx.input, &contours));
        return (findings, Vec::new());
    };
    findings.extend(too_tall(ctx, extent, options));
    findings.extend(empty_layers(ctx, extent));
    let (floating, erasures) = floating_parts(ctx, &bodies, &edge.held_past, options);
    findings.extend(floating);
    (findings, erasures)
}

/// The first and the last layer with solid.
#[derive(Clone, Copy, Debug)]
struct SolidExtent {
    first: usize,
    last: usize,
}

impl SolidExtent {
    fn of(regions: &PlateRegions) -> Option<Self> {
        let layers = regions.layers();
        let first = layers.iter().position(|layer| !layer.is_empty())?;
        let last = layers.iter().rposition(|layer| !layer.is_empty())?;
        Some(Self { first, last })
    }
}

fn region_of<'r>(regions: &'r PlateRegions, member: &RegionRef) -> &'r Region {
    &regions.layer(member.layer).solid[member.index as usize]
}

/// The largest solid region of a layer, the first one on a tie.
fn largest(regions: &[Region]) -> Option<&Region> {
    regions.iter().min_by_key(|region| Reverse(region.area_px))
}

/// `count` followed by `one` or `many`.
fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// `one` alone for a single item, otherwise `count` followed by `many`:
/// "layer is", "3 layers are".
fn counted(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        one.to_string()
    } else {
        format!("{count} {many}")
    }
}

/// A volume in cubic millimetres with enough decimals to stay non-zero for
/// a few pixels.
fn volume_text(mm3: f64) -> String {
    if mm3 >= 0.1 {
        format!("{mm3:.2}")
    } else if mm3 >= 0.001 {
        format!("{mm3:.4}")
    } else {
        format!("{mm3:.6}")
    }
}

fn pixel_centre(pixel: [u32; 2]) -> [f64; 2] {
    [f64::from(pixel[0]) + 0.5, f64::from(pixel[1]) + 0.5]
}

// ---------------------------------------------------------------------------
// The contours of every layer.
// ---------------------------------------------------------------------------

/// What the contours of one layer hold, as far as the output checks read
/// them.
#[derive(Clone, Debug, Default)]
struct LayerContours {
    /// The centre of the first polygon that stays within the plate.
    inside: Option<Point2D>,
    /// Whether a model polygon stays within the plate.
    inside_model: bool,
    /// Whether a support polygon stays within the plate.
    inside_support: bool,
    /// The polygons that reach past the plate, leaf by leaf.
    overhangs: Vec<Overhang>,
}

/// One polygon whose outer ring reaches past the edge of the plate.
#[derive(Clone, Copy, Debug)]
struct Overhang {
    /// Index of the leaf in [`ReviewInput::leaves`].
    leaf: usize,
    /// Index of the polygon in the leaf's layer.
    polygon: usize,
    /// Millimetres past the plate, the most on any side.
    past_mm: f64,
    /// A vertex that reaches that far, in plate millimetres.
    vertex: Point2D,
    /// The outer ring's bounding box, `[min x, min y, max x, max y]` in
    /// plate millimetres.
    bounds: [f64; 4],
}

impl Overhang {
    fn polygon<'a>(&self, input: &ReviewInput<'a>, layer: usize) -> &'a Polygon {
        &input.leaves()[self.leaf].stack.layers()[layer].polygons()[self.polygon]
    }
}

/// The vertices of `ring` reaching farthest towards each side of the plate:
/// `[min x, max x, min y, max y]` in plate millimetres, the first on a tie.
fn extremes(ring: &ClosedRing) -> Option<[Point2D; 4]> {
    let mut vertices = ring.vertices().iter().copied();
    let first = vertices.next()?;
    let mut ext = [first; 4];
    for v in vertices {
        if v.x < ext[0].x {
            ext[0] = v;
        }
        if v.x > ext[1].x {
            ext[1] = v;
        }
        if v.y < ext[2].y {
            ext[2] = v;
        }
        if v.y > ext[3].y {
            ext[3] = v;
        }
    }
    Some(ext)
}

/// Every layer's contours, measured against the plate. Holes lie inside
/// their outer ring, so only outer rings are read.
fn scan_contours(input: &ReviewInput<'_>, tolerance_mm: f64) -> Vec<LayerContours> {
    let half_x = 0.5 * input.pixel_map().plate_x().value();
    let half_y = 0.5 * input.pixel_map().plate_y().value();
    (0..input.layer_count())
        .into_par_iter()
        .map(|layer| {
            let mut contours = LayerContours::default();
            for (leaf, review_leaf) in input.leaves().iter().enumerate() {
                let polygons = review_leaf.stack.layers()[layer].polygons();
                for (polygon, shape) in polygons.iter().enumerate() {
                    let Some([min_x, max_x, min_y, max_y]) = extremes(shape.outer()) else {
                        continue;
                    };
                    let (past_mm, vertex) = [
                        (-half_x - min_x.x, min_x),
                        (max_x.x - half_x, max_x),
                        (-half_y - min_y.y, min_y),
                        (max_y.y - half_y, max_y),
                    ]
                    .into_iter()
                    .reduce(|best, side| if side.0 > best.0 { side } else { best })
                    .expect("four sides");
                    let bounds = [min_x.x, min_y.y, max_x.x, max_y.y];
                    if past_mm > tolerance_mm {
                        contours.overhangs.push(Overhang {
                            leaf,
                            polygon,
                            past_mm,
                            vertex,
                            bounds,
                        });
                        continue;
                    }
                    contours.inside.get_or_insert_with(|| {
                        Point2D::new(0.5 * (bounds[0] + bounds[2]), 0.5 * (bounds[1] + bounds[3]))
                    });
                    match review_leaf.role {
                        Role::Model => contours.inside_model = true,
                        Role::Support => contours.inside_support = true,
                    }
                }
            }
            contours
        })
        .collect()
}

// ---------------------------------------------------------------------------
// One polygon's pixels, on and off the screen.
// ---------------------------------------------------------------------------

/// A stretch of one row of the printer's pixel grid extended past the
/// screen in every direction: columns `x0..x1` of row `row`, which may lie
/// off the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    row: i64,
    x0: i64,
    x1: i64,
}

/// The pixels `polygon` alone lights in rows `rows` of the grid of
/// `pixel_map` extended past the screen, sorted by row and column. The rule
/// is the slicer's binary rasteriser's, with the same arithmetic: each row
/// is sampled at its centre, and a pixel is lit when the non-zero winding
/// count at its centre is non-zero. On the screen these are exactly the
/// pixels the polygon lights in the printer file. The work grows with the
/// rows scanned, so callers bound them.
fn polygon_spans(polygon: &Polygon, pixel_map: &PixelMap, rows: Range<i64>) -> Vec<Span> {
    // (row, crossing x, winding step)
    let mut crossings: Vec<(i64, f64, i32)> = Vec::new();
    for ring in iter::once(polygon.outer()).chain(polygon.holes()) {
        let points: Vec<(f64, f64)> = ring
            .vertices()
            .iter()
            .map(|v| pixel_map.mm_to_pixel(*v))
            .collect();
        if points.len() < 2 {
            continue;
        }
        for (i, &(ax, ay)) in points.iter().enumerate() {
            let (bx, by) = points[(i + 1) % points.len()];
            if !(ay.is_finite() && by.is_finite()) {
                continue;
            }
            // The rows the edge can cross, one wider on each side; the
            // exact test below decides.
            let low = (ay.min(by).floor() as i64 - 1).max(rows.start);
            let high = (ay.max(by).floor() as i64 + 1).min(rows.end - 1);
            for row in low..=high {
                let y = row as f64 + 0.5;
                if (ay > y) != (by > y) {
                    let t = (y - ay) / (by - ay);
                    let x = (bx - ax).mul_add(t, ax);
                    crossings.push((row, x, if ay < by { 1 } else { -1 }));
                }
            }
        }
    }
    crossings.sort_by(|p, q| p.0.cmp(&q.0).then(p.1.total_cmp(&q.1)));
    let mut spans = Vec::new();
    let mut winding = 0;
    let mut start = 0;
    for (k, &(row, x, step)) in crossings.iter().enumerate() {
        if k > 0 && crossings[k - 1].0 != row {
            winding = 0;
        }
        // The first pixel whose centre is at or past the crossing.
        let column = (x - 0.5).ceil() as i64;
        if winding == 0 {
            start = column;
        }
        winding += step;
        if winding == 0 && column > start {
            spans.push(Span {
                row,
                x0: start,
                x1: column,
            });
        }
    }
    spans
}

/// The part of `spans` on a `width_px` x `height_px` screen, as runs.
fn screen_runs(spans: &[Span], width_px: u32, height_px: u32) -> Vec<Run> {
    spans
        .iter()
        .filter_map(|span| {
            let row = u32::try_from(span.row)
                .ok()
                .filter(|&row| row < height_px)?;
            let clamp = |x: i64| x.clamp(0, i64::from(width_px)) as u32;
            let (x0, x1) = (clamp(span.x0), clamp(span.x1));
            (x0 < x1).then(|| Run::new(row, x0, x1))
        })
        .collect()
}

/// Whether two span lists, each sorted by row and column, share a pixel.
fn spans_overlap(a: &[Span], b: &[Span]) -> bool {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (p, q) = (a[i], b[j]);
        if p.row < q.row || (p.row == q.row && p.x1 <= q.x0) {
            i += 1;
        } else if q.row < p.row || q.x1 <= p.x0 {
            j += 1;
        } else {
            return true;
        }
    }
    false
}

/// Whether two bounding boxes `[min x, min y, max x, max y]` meet.
fn bounds_meet(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

// ---------------------------------------------------------------------------
// Plate edge.
// ---------------------------------------------------------------------------

/// The part of one body that lies on the screen edge.
#[derive(Clone, Copy, Debug)]
struct EdgeContact {
    /// The body's lowest region on the edge.
    first: RegionRef,
    /// The body's highest layer on the edge.
    last_layer: usize,
    /// Layers on which the body touches the edge.
    layers: usize,
    model_px: u64,
    support_px: u64,
}

impl EdgeContact {
    fn of(regions: &PlateRegions, members: &[RegionRef]) -> Option<Self> {
        let mut touching = members
            .iter()
            .filter(|member| region_of(regions, member).touches_border);
        let first = *touching.next()?;
        let region = region_of(regions, &first);
        let mut contact = Self {
            first,
            last_layer: first.layer,
            layers: 1,
            model_px: region.model_px,
            support_px: region.support_px,
        };
        for member in touching {
            if member.layer != contact.last_layer {
                contact.layers += 1;
                contact.last_layer = member.layer;
            }
            let region = region_of(regions, member);
            contact.model_px += region.model_px;
            contact.support_px += region.support_px;
        }
        Some(contact)
    }
}

/// What the pixels of one layer tell the plate-edge finding.
#[derive(Clone, Debug, Default)]
struct LayerHits {
    /// For each overhang of the layer, the bodies its pixels belong to.
    lit: Vec<Vec<usize>>,
    /// The edge pixel of every body whose first contact is on this layer.
    edge_pixels: Vec<(usize, [u32; 2])>,
}

/// Rasterises layer `layer` once, if anything on it needs its pixels: the
/// bodies its overhangs light, and the edge pixels of the bodies that first
/// touch the edge on it (`starting`).
fn layer_hits(
    ctx: &ReviewContext<'_>,
    bodies: &Groups,
    contacts: &[Option<EdgeContact>],
    layer: usize,
    overhangs: &[Overhang],
    starting: &[usize],
) -> LayerHits {
    let (input, regions) = (ctx.input, ctx.regions);
    let (width_px, height_px) = (regions.width_px(), regions.height_px());
    let half_x = 0.5 * input.pixel_map().plate_x().value();
    let half_y = 0.5 * input.pixel_map().plate_y().value();
    let plate = [-half_x, -half_y, half_x, half_y];
    // Only a polygon whose box meets the plate can light a pixel.
    let on_plate: Vec<bool> = overhangs
        .iter()
        .map(|overhang| bounds_meet(overhang.bounds, plate))
        .collect();
    let mut hits = LayerHits {
        lit: vec![Vec::new(); overhangs.len()],
        edge_pixels: Vec::new(),
    };
    if starting.is_empty() && !on_plate.contains(&true) {
        return hits;
    }
    let pixels = LayerPixels::compute(input, layer);
    for ((overhang, lit), _) in overhangs
        .iter()
        .zip(&mut hits.lit)
        .zip(&on_plate)
        .filter(|(_, on_plate)| **on_plate)
    {
        let spans = polygon_spans(
            overhang.polygon(input, layer),
            input.pixel_map(),
            0..i64::from(height_px),
        );
        let runs = screen_runs(&spans, width_px, height_px);
        let (Some(top), Some(bottom)) = (runs.first(), runs.last()) else {
            continue;
        };
        // Only the layer's runs in the rows the polygon lights.
        let start = pixels.solid.partition_point(|run| run.row < top.row);
        let end = pixels.solid.partition_point(|run| run.row <= bottom.row);
        for_each_overlap(&runs, &pixels.solid[start..end], |_, j, _| {
            let index = pixels.solid_labels[start + j];
            let body = bodies.group_of(RegionRef { layer, index });
            if !lit.contains(&body) {
                lit.push(body);
            }
        });
    }
    let wanted: Vec<(u32, &Region)> = starting
        .iter()
        .map(|&body| {
            let first = contacts[body].expect("a body that touches the edge").first;
            (first.index, region_of(regions, &first))
        })
        .collect();
    let found = edge_pixels(&pixels, &wanted, width_px, height_px);
    hits.edge_pixels = starting.iter().copied().zip(found).collect();
    hits
}

/// Two overhangs joined into one part, each as `(layer, index on it)`.
type OverhangLink = ((usize, usize), (usize, usize));

/// The overhangs of the same or consecutive layers, of any leaf, that share
/// a pixel of the pixel grid extended past the screen, or whose bounding
/// boxes meet when one of them lights no pixel of that grid (a sliver).
/// Solid joins across leaves on the screen, so it does past it too. The
/// grid is extended by one screen height above and below the screen, which
/// bounds the work for absurdly large contours; polygons that lie only
/// farther out light nothing there and are joined by their boxes. Only
/// overhangs whose boxes meet are rasterised and compared, each once per
/// layer pair.
fn linked_overhangs(input: &ReviewInput<'_>, contours: &[LayerContours]) -> Vec<OverhangLink> {
    let height = i64::from(input.height_px());
    let rows = -height..2 * height;
    (0..contours.len())
        .into_par_iter()
        .flat_map_iter(|layer| {
            let below: &[Overhang] = match layer.checked_sub(1) {
                Some(lower) => &contours[lower].overhangs,
                None => &[],
            };
            // The overhangs of the layer below, then those of this one.
            let items: Vec<(usize, usize, &Overhang)> = below
                .iter()
                .enumerate()
                .map(|(k, overhang)| (layer - 1, k, overhang))
                .chain(
                    contours[layer]
                        .overhangs
                        .iter()
                        .enumerate()
                        .map(|(k, overhang)| (layer, k, overhang)),
                )
                .collect();
            let boxes: Vec<[f64; 4]> = items.iter().map(|item| item.2.bounds).collect();
            let mut spans: Vec<Option<Vec<Span>>> = vec![None; items.len()];
            let mut links = Vec::new();
            for (p, q) in meeting_boxes(&boxes, below.len()) {
                for k in [p, q] {
                    if spans[k].is_none() {
                        let (on, _, overhang) = items[k];
                        spans[k] = Some(polygon_spans(
                            overhang.polygon(input, on),
                            input.pixel_map(),
                            rows.clone(),
                        ));
                    }
                }
                let a = spans[p].as_deref().unwrap_or_default();
                let b = spans[q].as_deref().unwrap_or_default();
                if a.is_empty() || b.is_empty() || spans_overlap(a, b) {
                    links.push(((items[p].0, items[p].1), (items[q].0, items[q].1)));
                }
            }
            links
        })
        .collect()
}

/// The pairs `(p, q)`, `p < q`, of `boxes` (`[min x, min y, max x, max y]`)
/// that meet, with `q` at or after `split`: the boxes before `split` are
/// only paired with those from it on. A sweep along x, so boxes far apart
/// are never compared.
fn meeting_boxes(boxes: &[[f64; 4]], split: usize) -> Vec<(usize, usize)> {
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by(|&a, &b| boxes[a][0].total_cmp(&boxes[b][0]));
    let mut active: Vec<usize> = Vec::new();
    let mut pairs = Vec::new();
    for k in order {
        let b = boxes[k];
        active.retain(|&a| boxes[a][2] >= b[0]);
        for &a in &active {
            let (p, q) = (a.min(k), a.max(k));
            if q >= split && boxes[a][1] <= b[3] && b[1] <= boxes[a][3] {
                pairs.push((p, q));
            }
        }
        active.push(k);
    }
    pairs
}

/// One part at the edge of the plate: the bodies and overhangs joined into
/// it.
#[derive(Clone, Debug, Default)]
struct EdgePart {
    /// First and last layer it touches the edge or reaches past it.
    layers: Option<(usize, usize)>,
    /// Its bodies that touch the edge, lowest numbered first, with the
    /// layer they first touch it on.
    contacts: Vec<(usize, usize)>,
    /// Layers on which it touches the edge.
    touch_layers: usize,
    /// Layers on which it reaches past the plate, ascending.
    past_layers: Vec<usize>,
    /// The farthest past the plate on any layer.
    past_mm: f64,
    /// Its farthest vertex past the plate on the lowest layer it reaches
    /// past the plate on: that layer, how far, and the vertex.
    lowest_past: Option<(usize, f64, Point2D)>,
    model: bool,
    support: bool,
}

impl EdgePart {
    fn widen(&mut self, first: usize, last: usize) {
        self.layers = Some(match self.layers {
            Some((a, b)) => (a.min(first), b.max(last)),
            None => (first, last),
        });
    }

    fn add_contact(&mut self, body: usize, contact: &EdgeContact) {
        self.widen(contact.first.layer, contact.last_layer);
        self.contacts.push((body, contact.first.layer));
        self.touch_layers += contact.layers;
        self.model |= contact.model_px > 0;
        self.support |= contact.support_px > 0;
    }

    /// Adds an overhang of layer `layer`. Overhangs come in layer order.
    fn add_overhang(&mut self, layer: usize, overhang: &Overhang, role: Role) {
        self.widen(layer, layer);
        if self.past_layers.last() != Some(&layer) {
            self.past_layers.push(layer);
        }
        self.past_mm = self.past_mm.max(overhang.past_mm);
        match self.lowest_past {
            Some((lowest, past_mm, _)) if lowest != layer || past_mm >= overhang.past_mm => {}
            _ => self.lowest_past = Some((layer, overhang.past_mm, overhang.vertex)),
        }
        match role {
            Role::Model => self.model = true,
            Role::Support => self.support = true,
        }
    }

    fn finding(&self, input: &ReviewInput<'_>, edge_pixels: &[Option<[u32; 2]>]) -> Finding {
        let (first, last) = self.layers.expect("a part at the edge has layers");
        let focus = self
            .contacts
            .iter()
            .filter(|(_, layer)| *layer == first)
            .find_map(|(body, _)| edge_pixels[*body])
            .map(|pixel| input.focus(first, pixel_centre(pixel)))
            .or_else(|| {
                self.lowest_past
                    .filter(|(layer, _, _)| *layer == first)
                    .map(|(_, _, v)| PlatePoint::new(v.x, v.y, input.layers()[first].sample_mm))
            })
            .expect("a part touches the edge or reaches past it on its first layer");
        let scope = RoleScope::from_counts(u64::from(self.model), u64::from(self.support));
        if !self.past_layers.is_empty() {
            Finding::new(
                CheckKind::PlateEdge,
                Severity::Error,
                first,
                last,
                focus,
                scope,
                "Part cut by the plate edge",
                format!(
                    "It reaches up to {:.2} mm past the edge of the plate on {}, and nothing beyond the screen is printed.",
                    self.past_mm,
                    plural(self.past_layers.len(), "layer", "layers")
                ),
            )
        } else {
            Finding::new(
                CheckKind::PlateEdge,
                Severity::Warning,
                first,
                last,
                focus,
                scope,
                "Part touches the screen edge",
                format!(
                    "It lights the outermost pixels of the screen on {}: nothing is cut off, but it has no margin left.",
                    plural(self.touch_layers, "layer", "layers")
                ),
            )
        }
    }
}

/// What the plate-edge check found.
struct PlateEdge {
    findings: Vec<Finding>,
    /// For each body, whether its part reaches past the plate on the layer
    /// right below the body's lowest layer: what holds it lies beyond the
    /// screen.
    held_past: Vec<bool>,
}

/// One finding per part at the edge of the plate, and per part of contours
/// past the plate that joins no body.
fn plate_edge(ctx: &ReviewContext<'_>, bodies: &Groups, contours: &[LayerContours]) -> PlateEdge {
    let (input, regions) = (ctx.input, ctx.regions);
    let contacts: Vec<Option<EdgeContact>> = bodies
        .iter()
        .map(|members| EdgeContact::of(regions, members))
        .collect();
    let mut starting: Vec<Vec<usize>> = vec![Vec::new(); input.layer_count()];
    for (body, contact) in contacts.iter().enumerate() {
        if let Some(contact) = contact {
            starting[contact.first.layer].push(body);
        }
    }
    let hits: Vec<LayerHits> = (0..input.layer_count())
        .into_par_iter()
        .map(|layer| {
            layer_hits(
                ctx,
                bodies,
                &contacts,
                layer,
                &contours[layer].overhangs,
                &starting[layer],
            )
        })
        .collect();
    let links = linked_overhangs(input, contours);

    // Bodies are elements 0..bodies.len(); the overhangs of layer `k`
    // follow from `offsets[k]`.
    let mut offsets = Vec::with_capacity(contours.len());
    let mut total = bodies.len();
    for layer in contours {
        offsets.push(total);
        total += layer.overhangs.len();
    }
    let mut sets = UnionFind::new(total);
    for (layer, layer_hits) in hits.iter().enumerate() {
        for (k, lit) in layer_hits.lit.iter().enumerate() {
            for &body in lit {
                sets.union(body, offsets[layer] + k);
            }
        }
    }
    for &((a_layer, a), (b_layer, b)) in &links {
        sets.union(offsets[a_layer] + a, offsets[b_layer] + b);
    }
    let mut has_body = vec![false; total];
    for body in 0..bodies.len() {
        has_body[sets.find(body)] = true;
    }

    let mut edge_pixels: Vec<Option<[u32; 2]>> = vec![None; bodies.len()];
    for layer_hits in &hits {
        for &(body, pixel) in &layer_hits.edge_pixels {
            edge_pixels[body] = Some(pixel);
        }
    }
    let mut parts: BTreeMap<usize, EdgePart> = BTreeMap::new();
    for (body, contact) in contacts.iter().enumerate() {
        if let Some(contact) = contact {
            parts
                .entry(sets.find(body))
                .or_default()
                .add_contact(body, contact);
        }
    }
    let mut lost: Vec<(usize, usize, Overhang)> = Vec::new();
    for (layer, layer_contours) in contours.iter().enumerate() {
        for (k, overhang) in layer_contours.overhangs.iter().enumerate() {
            let root = sets.find(offsets[layer] + k);
            if has_body[root] {
                let role = input.leaves()[overhang.leaf].role;
                parts
                    .entry(root)
                    .or_default()
                    .add_overhang(layer, overhang, role);
            } else {
                lost.push((root, layer, *overhang));
            }
        }
    }
    let held_past = bodies
        .iter()
        .enumerate()
        .map(|(body, members)| {
            let Some(below) = members[0].layer.checked_sub(1) else {
                return false;
            };
            parts
                .get(&sets.find(body))
                .is_some_and(|part| part.past_layers.binary_search(&below).is_ok())
        })
        .collect();
    let mut findings: Vec<Finding> = parts
        .values()
        .map(|part| part.finding(input, &edge_pixels))
        .collect();
    findings.extend(outside_plate(input, &lost));
    PlateEdge {
        findings,
        held_past,
    }
}

/// For each solid region of `wanted` (its index in the layer and its
/// statistics, each region once), its pixel on the screen's outermost row
/// or column that is nearest to its centroid, the first in scan order on a
/// tie. One pass over the layer's runs. Falls back on the region's inside
/// pixel, which cannot happen for a region that touches the edge.
fn edge_pixels(
    pixels: &LayerPixels,
    wanted: &[(u32, &Region)],
    width_px: u32,
    height_px: u32,
) -> Vec<[u32; 2]> {
    let mut slot = vec![usize::MAX; pixels.solid_count];
    for (k, (index, _)) in wanted.iter().enumerate() {
        slot[*index as usize] = k;
    }
    let mut best: Vec<(f64, [u32; 2])> = wanted
        .iter()
        .map(|(_, region)| (f64::INFINITY, region.inside_px))
        .collect();
    for (run, &label) in pixels.solid.iter().zip(&pixels.solid_labels) {
        let k = slot[label as usize];
        if k == usize::MAX {
            continue;
        }
        let [cx, cy] = wanted[k].1.centroid_px;
        let best = &mut best[k];
        let mut consider = |x: u32, y: u32| {
            let dx = f64::from(x) + 0.5 - cx;
            let dy = f64::from(y) + 0.5 - cy;
            let distance = dx.mul_add(dx, dy * dy);
            if distance < best.0 {
                *best = (distance, [x, y]);
            }
        };
        if run.row == 0 || run.row + 1 == height_px {
            let x = (cx.floor().max(0.0) as u32).clamp(run.x0, run.x1 - 1);
            consider(x, run.row);
        } else {
            if run.x0 == 0 {
                consider(0, run.row);
            }
            if run.x1 == width_px {
                consider(width_px - 1, run.row);
            }
        }
    }
    best.into_iter().map(|(_, pixel)| pixel).collect()
}

/// Polygons past the plate joined into one part that joins no body.
struct OutsidePart {
    /// Its leaves, as indices in [`ReviewInput::leaves`], ascending.
    leaves: Vec<usize>,
    first: usize,
    last: usize,
    /// Layers it has a polygon on.
    layers: usize,
    /// The farthest past the plate on any of its layers.
    past_mm: f64,
    /// The farthest past the plate on its first layer, and where.
    first_past_mm: f64,
    focus: PlatePoint,
}

/// How a leaf is named in a finding: its name in quotes, or what it is.
fn leaf_name(name: Option<&str>, role: Role) -> String {
    match (name, role) {
        (Some(name), _) => format!("'{name}'"),
        (None, Role::Model) => "a model".to_string(),
        (None, Role::Support) => "a support".to_string(),
    }
}

/// One finding per part of `lost`, the overhangs that join no body, each
/// as `(its part, its layer, the overhang)` in layer order.
fn outside_plate(input: &ReviewInput<'_>, lost: &[(usize, usize, Overhang)]) -> Vec<Finding> {
    let mut parts: Vec<OutsidePart> = Vec::new();
    let mut index_of: BTreeMap<usize, usize> = BTreeMap::new();
    for &(root, layer, overhang) in lost {
        let focus = PlatePoint::new(
            overhang.vertex.x,
            overhang.vertex.y,
            input.layers()[layer].sample_mm,
        );
        let Some(&index) = index_of.get(&root) else {
            index_of.insert(root, parts.len());
            parts.push(OutsidePart {
                leaves: vec![overhang.leaf],
                first: layer,
                last: layer,
                layers: 1,
                past_mm: overhang.past_mm,
                first_past_mm: overhang.past_mm,
                focus,
            });
            continue;
        };
        let part = &mut parts[index];
        if let Err(at) = part.leaves.binary_search(&overhang.leaf) {
            part.leaves.insert(at, overhang.leaf);
        }
        if layer != part.last {
            part.layers += 1;
            part.last = layer;
        }
        part.past_mm = part.past_mm.max(overhang.past_mm);
        if part.first == layer && overhang.past_mm > part.first_past_mm {
            part.first_past_mm = overhang.past_mm;
            part.focus = focus;
        }
    }
    let leaves = input.leaves();
    parts
        .into_iter()
        .map(|part| {
            let has = |role: Role| part.leaves.iter().any(|&leaf| leaves[leaf].role == role);
            // Models first, then supports, each in leaf order.
            let mut names: Vec<String> = [Role::Model, Role::Support]
                .into_iter()
                .flat_map(|role| {
                    part.leaves
                        .iter()
                        .filter(move |&&leaf| leaves[leaf].role == role)
                        .map(move |&leaf| leaf_name(leaves[leaf].name, role))
                })
                .collect();
            let verb = if names.len() == 1 { "reaches" } else { "reach" };
            let last_name = names.pop().expect("a part has a leaf");
            let mut who = if names.is_empty() {
                last_name
            } else {
                format!("{} and {last_name}", names.join(", "))
            };
            // Unnamed leaves start lower case; the sentence does not.
            if let Some(initial) = who.get(..1).map(str::to_uppercase) {
                who.replace_range(..1, &initial);
            }
            Finding::new(
                CheckKind::PlateEdge,
                Severity::Error,
                part.first,
                part.last,
                part.focus,
                RoleScope::from_counts(u64::from(has(Role::Model)), u64::from(has(Role::Support))),
                "Part outside the printable area",
                format!(
                    "{who} {verb} up to {:.2} mm past the edge of the plate on {}, and nothing beyond the screen is printed.",
                    part.past_mm,
                    plural(part.layers, "layer", "layers")
                ),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Too tall.
// ---------------------------------------------------------------------------

/// One finding when the highest solid layer ends above the printer's
/// maximum height.
fn too_tall(
    ctx: &ReviewContext<'_>,
    extent: SolidExtent,
    options: &OutputOptions,
) -> Option<Finding> {
    let (input, regions) = (ctx.input, ctx.regions);
    let layers = input.layers();
    let max_z = input.max_z_mm();
    let limit = max_z + options.height_tolerance_mm;
    let top = layers[extent.last].top_mm;
    if top <= limit {
        return None;
    }
    let first = (extent.first..=extent.last)
        .find(|&layer| layers[layer].top_mm > limit && !regions.layer(layer).is_empty())?;
    let (mut model_px, mut support_px) = (0, 0);
    for layer in first..=extent.last {
        for region in &regions.layer(layer).solid {
            model_px += region.model_px;
            support_px += region.support_px;
        }
    }
    let region = largest(&regions.layer(first).solid)?;
    Some(Finding::new(
        CheckKind::TooTall,
        Severity::Error,
        first,
        extent.last,
        input.focus(first, region.inside_point()),
        RoleScope::from_counts(model_px, support_px),
        "Print taller than the printer",
        format!(
            "The print reaches {top:.2} mm, {:.2} mm above the printer's maximum height of {max_z:.2} mm, so its top {} cannot be printed.",
            top - max_z,
            counted(extent.last - first + 1, "layer", "layers")
        ),
    ))
}

// ---------------------------------------------------------------------------
// Empty layers.
// ---------------------------------------------------------------------------

/// One finding per run of consecutive empty layers: before the first solid
/// layer, between solid layers, and after the last one.
fn empty_layers(ctx: &ReviewContext<'_>, extent: SolidExtent) -> Vec<Finding> {
    let (input, regions) = (ctx.input, ctx.regions);
    let layers = input.layers();
    // Focus on the solid next to the empty layers, at the first empty
    // layer's height.
    let focus = |empty: usize, solid: usize| {
        let region = largest(&regions.layer(solid).solid).expect("a layer with solid");
        input.focus(empty, region.inside_point())
    };
    let mut findings = Vec::new();
    if extent.first > 0 {
        let count = extent.first;
        findings.push(Finding::new(
            CheckKind::EmptyLayer,
            Severity::Error,
            0,
            count - 1,
            focus(0, extent.first),
            RoleScope::Both,
            "Nothing on the build plate",
            format!(
                "The first {} empty, so the first solid layer cures {:.2} mm above the plate with nothing to stick to.",
                counted(count, "layer is", "layers are"),
                layers[extent.first].bottom_mm
            ),
        ));
    }
    let mut layer = extent.first + 1;
    while layer < extent.last {
        if !regions.layer(layer).is_empty() {
            layer += 1;
            continue;
        }
        let start = layer;
        while regions.layer(layer).is_empty() {
            layer += 1;
        }
        let gap_mm = layers[layer].bottom_mm - layers[start].bottom_mm;
        findings.push(Finding::new(
            CheckKind::EmptyLayer,
            Severity::Error,
            start,
            layer - 1,
            focus(start, layer),
            RoleScope::Both,
            "Gap in the print",
            format!(
                "{} a {gap_mm:.2} mm gap, so everything above it has nothing to rest on. The printer file still lights a pixel on every layer, so the printer does not stall on them.",
                plural(layer - start, "empty layer leaves", "empty layers leave")
            ),
        ));
    }
    let top = input.layer_count() - 1;
    let count = top - extent.last;
    if count > 0 {
        let first = extent.last + 1;
        let height_mm = layers[top].top_mm - layers[first].bottom_mm;
        findings.push(Finding::new(
            CheckKind::EmptyLayer,
            Severity::Info,
            first,
            top,
            focus(first, extent.last),
            RoleScope::Both,
            if count == 1 {
                "Empty layer at the top"
            } else {
                "Empty layers at the top"
            },
            format!(
                "The last {} empty ({height_mm:.2} mm): {} nothing, so the printer file leaves {} out.",
                counted(count, "layer is", "layers are"),
                if count == 1 { "it prints" } else { "they print" },
                if count == 1 { "it" } else { "them" },
            ),
        ));
    }
    findings
}

/// One finding when the plate has contours within the plate but not a
/// single lit pixel on any layer. Contours past the plate are already
/// reported as outside the printable area.
fn nothing_to_print(input: &ReviewInput<'_>, contours: &[LayerContours]) -> Option<Finding> {
    let centre = contours.iter().find_map(|layer| layer.inside)?;
    let model = contours.iter().any(|layer| layer.inside_model);
    let support = contours.iter().any(|layer| layer.inside_support);
    let count = input.layer_count();
    Some(Finding::new(
        CheckKind::EmptyLayer,
        Severity::Error,
        0,
        count - 1,
        PlatePoint::new(centre.x, centre.y, input.layers()[0].sample_mm),
        RoleScope::from_counts(u64::from(model), u64::from(support)),
        "Nothing to print",
        format!(
            "{} empty: the parts on the plate are too small or too thin to light a single pixel, so the printer file prints nothing.",
            if count == 1 {
                "The only layer is".to_string()
            } else {
                format!("All {count} layers are")
            }
        ),
    ))
}

// ---------------------------------------------------------------------------
// Floating parts.
// ---------------------------------------------------------------------------

/// One finding per printed body that never reaches the plate, unless it
/// starts right above an empty layer, which the empty-layer finding
/// explains, or right above where its part is cut by the plate edge
/// (`held_past`), which the plate-edge finding explains.
fn floating_parts(
    ctx: &ReviewContext<'_>,
    bodies: &Groups,
    held_past: &[bool],
    options: &OutputOptions,
) -> (Vec<Finding>, Vec<CavityFill>) {
    let (input, regions) = (ctx.input, ctx.regions);
    let layers = input.layers();
    let pixel_mm2 = input.pixel_area_mm2();
    let mut findings = Vec::new();
    let mut specks: Vec<&[RegionRef]> = Vec::new();
    // The body of each finding, for its footprint (#409).
    let mut found: Vec<&[RegionRef]> = Vec::new();
    for (members, &held_past) in bodies.iter().zip(held_past) {
        let bottom = members[0].layer;
        if bottom < PLATE_LAYERS || regions.layer(bottom - 1).is_empty() || held_past {
            continue;
        }
        let (mut model_px, mut support_px, mut volume_mm3) = (0, 0, 0.0);
        for member in members {
            let region = region_of(regions, member);
            model_px += region.model_px;
            support_px += region.support_px;
            volume_mm3 += region.area_px as f64 * pixel_mm2 * layers[member.layer].height_mm();
        }
        let base = members
            .iter()
            .take_while(|member| member.layer == bottom)
            .map(|member| region_of(regions, member))
            .min_by_key(|region| Reverse(region.area_px))
            .expect("a body has a bottom region");
        let focus = input.focus(bottom, base.inside_point());
        let last = members[members.len() - 1].layer;
        let scope = RoleScope::from_counts(model_px, support_px);
        let height_mm = layers[bottom].bottom_mm;
        if volume_mm3 < options.speck_volume_mm3 {
            findings.push(Finding::new(
                CheckKind::FloatingPart,
                Severity::Warning,
                bottom,
                last,
                focus,
                scope,
                "Floating speck",
                format!(
                    "This {} mm³ speck starts {height_mm:.2} mm above the build plate with nothing below it. It is too small to hold with a support: it may cure as loose debris in the vat, or not at all.",
                    volume_text(volume_mm3)
                ),
            ).with_fix(SuggestedFix::Erase { at: focus, speck: specks.len() }));
            specks.push(members);
            found.push(members);
            continue;
        }
        let finding = Finding::new(
            CheckKind::FloatingPart,
            Severity::Error,
            bottom,
            last,
            focus,
            scope,
            "Floating part",
            format!(
                "This {} mm³ part starts {height_mm:.2} mm above the build plate and never joins anything standing on the plate, so it will not stay attached.",
                volume_text(volume_mm3)
            ),
        );
        findings.push(if base.model_px > 0 {
            finding.with_fix(SuggestedFix::AddSupport {
                at: input.underside(bottom, focus),
            })
        } else {
            finding
        });
        found.push(members);
    }
    let erasures = speck_erasures(input, &specks);
    let findings = findings
        .into_iter()
        .zip(body_footprints(input, &found))
        .map(|(finding, footprint)| finding.with_footprint(Arc::new(footprint)))
        .collect();
    (findings, erasures)
}

/// The pixels of each body of `bodies` (its member regions), on at most
/// [`MAX_FOOTPRINT_LAYERS`] layers from its first: what its finding
/// highlights (#409). Each layer is rasterised once, in parallel.
fn body_footprints(input: &ReviewInput<'_>, bodies: &[&[RegionRef]]) -> Vec<CavityFill> {
    let mut wanted: Vec<(usize, usize, u32)> = bodies
        .iter()
        .enumerate()
        .flat_map(|(body, members)| {
            let first = members.first().map_or(0, |m| m.layer);
            members
                .iter()
                .filter(move |m| m.layer < first + MAX_FOOTPRINT_LAYERS)
                .map(move |m| (m.layer, body, m.index))
        })
        .collect();
    wanted.sort_unstable();
    let by_layer: Vec<&[(usize, usize, u32)]> = wanted.chunk_by(|a, b| a.0 == b.0).collect();
    let found: Vec<Vec<(usize, usize, Vec<Run>)>> = by_layer
        .par_iter()
        .map(|members| {
            let layer = members[0].0;
            let pixels = LayerPixels::compute(input, layer);
            members
                .chunk_by(|a, b| a.1 == b.1)
                .map(|regions| {
                    let own: Vec<Run> = pixels
                        .solid
                        .iter()
                        .zip(&pixels.solid_labels)
                        .filter(|(_, label)| regions.iter().any(|r| r.2 == **label))
                        .map(|(run, _)| *run)
                        .collect();
                    (layer, regions[0].1, own)
                })
                .collect()
        })
        .collect();
    let mut out: Vec<Vec<(usize, Vec<Run>)>> = vec![Vec::new(); bodies.len()];
    for (layer, body, runs) in found.into_iter().flatten() {
        out[body].push((layer, runs));
    }
    out.into_iter()
        .map(|layers| CavityFill::from_runs(input, layers))
        .collect()
}

/// The pixels that erase each speck of `specks` (its member regions): its
/// own, grown a pixel on every side. The margin is air on the speck's
/// layers, solid being 8-connected; where two specks' margins meet, the
/// pixel goes to the first one, so no pixel is erased twice. Each layer any
/// speck touches is rasterised once, in parallel.
fn speck_erasures(input: &ReviewInput<'_>, specks: &[&[RegionRef]]) -> Vec<CavityFill> {
    if specks.is_empty() {
        return Vec::new();
    }
    let mut wanted: Vec<(usize, usize, u32)> = specks
        .iter()
        .enumerate()
        .flat_map(|(speck, members)| {
            members
                .iter()
                .map(move |member| (member.layer, speck, member.index))
        })
        .collect();
    wanted.sort_unstable();
    let by_layer: Vec<&[(usize, usize, u32)]> = wanted.chunk_by(|a, b| a.0 == b.0).collect();
    let (width, height) = (input.width_px(), input.height_px());
    let found: Vec<Vec<(usize, usize, Vec<Run>)>> = by_layer
        .par_iter()
        .map(|members| {
            let layer = members[0].0;
            let pixels = LayerPixels::compute(input, layer);
            let mut claimed: Vec<Run> = Vec::new();
            members
                .chunk_by(|a, b| a.1 == b.1)
                .map(|regions| {
                    let own: Vec<Run> = pixels
                        .solid
                        .iter()
                        .zip(&pixels.solid_labels)
                        .filter(|(_, label)| regions.iter().any(|r| r.2 == **label))
                        .map(|(run, _)| *run)
                        .collect();
                    let grown =
                        thin::difference(&thin::dilate_square(&own, 1, width, height), &claimed);
                    claimed = runs::union(&claimed, &grown);
                    (layer, regions[0].1, grown)
                })
                .collect()
        })
        .collect();
    let mut out: Vec<Vec<(usize, Vec<Run>)>> = vec![Vec::new(); specks.len()];
    for (layer, speck, runs) in found.into_iter().flatten() {
        out[speck].push((layer, runs));
    }
    out.into_iter()
        .map(|layers| CavityFill::from_runs(input, layers))
        .collect()
}

#[cfg(test)]
#[path = "tests_output.rs"]
mod tests;
