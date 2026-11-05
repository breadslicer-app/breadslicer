/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Output checks (#93): plate edges, maximum height and empty layers.
//!
//! Reports [`CheckKind::PlateEdge`], [`CheckKind::TooTall`] and
//! [`CheckKind::EmptyLayer`] findings, all read from the sweep's tables plus,
//! for the plate edge and the empty plate, the models' vector contours.
//! Supports are data (#432): the layers they stand on count as lit, and the
//! box they take is compared with the plate:
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
//! - **Supports past the plate** (error). The box the supports take reaches
//!   past the plate's edge or above the printer's maximum height.
//!
//! Joining solid within a layer follows the shared regions, which are
//! 8-connected: a part that touches a body only by a corner pixel belongs to
//! it. A body that never reaches the plate is an island (see
//! [`crate::checks::islands`]).

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::iter;
use std::ops::Range;

use rayon::prelude::*;
use sc_sl1cer::contour::{ClosedRing, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::raster::pixel_map::PixelMap;

use crate::finding::{CheckKind, Finding, PlatePoint, RoleScope, Severity};
use crate::input::ReviewInput;
use crate::regions::{LayerPixels, PlateRegions, Region, SweepOptions};
use crate::runner::ReviewContext;
use crate::runs::{Run, for_each_overlap};
use crate::union_find::{Groups, RegionRef, UnionFind};

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
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            edge_tolerance_mm: 1e-3,
            height_tolerance_mm: 1e-3,
        }
    }
}

/// Finds the output problems of the plate.
pub fn run(ctx: &ReviewContext<'_>, options: &OutputOptions, sweep: &SweepOptions) -> Vec<Finding> {
    let contours = scan_contours(ctx.input, options.edge_tolerance_mm);
    let bodies = ctx.regions.group_solid(|_, _| true);
    let mut findings = plate_edge(ctx, &bodies, &contours, sweep);
    findings.extend(supports_past_the_plate(ctx.input, options));
    let Some(extent) = SolidExtent::of(ctx) else {
        findings.extend(nothing_to_print(ctx.input, &contours));
        return findings;
    };
    findings.extend(too_tall(ctx, extent, options));
    findings.extend(empty_layers(ctx, extent));
    findings
}

/// The first and the last layer with solid.
#[derive(Clone, Copy, Debug)]
struct SolidExtent {
    first: usize,
    last: usize,
}

impl SolidExtent {
    fn of(ctx: &ReviewContext<'_>) -> Option<Self> {
        let count = ctx.input.layer_count();
        let first = (0..count).find(|&layer| !ctx.layer_is_empty(layer))?;
        let last = (0..count).rfind(|&layer| !ctx.layer_is_empty(layer))?;
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
}

impl EdgeContact {
    fn of(regions: &PlateRegions, members: &[RegionRef]) -> Option<Self> {
        let mut touching = members
            .iter()
            .filter(|member| region_of(regions, member).touches_border);
        let first = *touching.next()?;
        let mut contact = Self {
            first,
            last_layer: first.layer,
            layers: 1,
        };
        for member in touching {
            if member.layer != contact.last_layer {
                contact.layers += 1;
                contact.last_layer = member.layer;
            }
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
    sweep: &SweepOptions,
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
    let pixels = LayerPixels::compute(input, layer, sweep);
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
    }

    /// Adds an overhang of layer `layer`. Overhangs come in layer order.
    fn add_overhang(&mut self, layer: usize, overhang: &Overhang) {
        self.widen(layer, layer);
        if self.past_layers.last() != Some(&layer) {
            self.past_layers.push(layer);
        }
        self.past_mm = self.past_mm.max(overhang.past_mm);
        match self.lowest_past {
            Some((lowest, past_mm, _)) if lowest != layer || past_mm >= overhang.past_mm => {}
            _ => self.lowest_past = Some((layer, overhang.past_mm, overhang.vertex)),
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
        let scope = RoleScope::Model;
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

/// One finding per part at the edge of the plate, and per part of contours
/// past the plate that joins no body.
fn plate_edge(
    ctx: &ReviewContext<'_>,
    bodies: &Groups,
    contours: &[LayerContours],
    sweep: &SweepOptions,
) -> Vec<Finding> {
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
                sweep,
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
                parts.entry(root).or_default().add_overhang(layer, overhang);
            } else {
                lost.push((root, layer, *overhang));
            }
        }
    }
    let mut findings: Vec<Finding> = parts
        .values()
        .map(|part| part.finding(input, &edge_pixels))
        .collect();
    findings.extend(outside_plate(input, &lost));
    findings
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
fn leaf_name(name: Option<&str>) -> String {
    name.map_or_else(|| "a model".to_string(), |name| format!("'{name}'"))
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
            let mut names: Vec<String> = part
                .leaves
                .iter()
                .map(|&leaf| leaf_name(leaves[leaf].name))
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
                RoleScope::Model,
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
    let input = ctx.input;
    let layers = input.layers();
    let max_z = input.max_z_mm();
    let limit = max_z + options.height_tolerance_mm;
    let top = layers[extent.last].top_mm;
    if top <= limit {
        return None;
    }
    let first = (extent.first..=extent.last)
        .find(|&layer| layers[layer].top_mm > limit && !ctx.layer_is_empty(layer))?;
    Some(Finding::new(
        CheckKind::TooTall,
        Severity::Error,
        first,
        extent.last,
        input.focus(first, point_on(ctx, first)),
        RoleScope::Model,
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
    let input = ctx.input;
    let layers = input.layers();
    // Focus on the solid next to the empty layers, at the first empty
    // layer's height.
    let focus = |empty: usize, solid: usize| input.focus(empty, point_on(ctx, solid));
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
        if !ctx.layer_is_empty(layer) {
            layer += 1;
            continue;
        }
        let start = layer;
        while ctx.layer_is_empty(layer) {
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
    let count = input.layer_count();
    Some(Finding::new(
        CheckKind::EmptyLayer,
        Severity::Error,
        0,
        count - 1,
        PlatePoint::new(centre.x, centre.y, input.layers()[0].sample_mm),
        RoleScope::Model,
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

/// A point of layer `layer` to look at: inside its largest solid region, or
/// the middle of the screen on a layer only supports stand on.
fn point_on(ctx: &ReviewContext<'_>, layer: usize) -> [f64; 2] {
    largest(&ctx.regions.layer(layer).solid).map_or_else(
        || {
            [
                0.5 * f64::from(ctx.input.width_px()),
                0.5 * f64::from(ctx.input.height_px()),
            ]
        },
        Region::inside_point,
    )
}

// ---------------------------------------------------------------------------
// Supports past the plate.
// ---------------------------------------------------------------------------

/// One finding when the box the supports take reaches past the plate or
/// above the printer's maximum height. Their geometry is not read: the box
/// is what the input says of them.
fn supports_past_the_plate(input: &ReviewInput<'_>, options: &OutputOptions) -> Option<Finding> {
    let [x0, y0, x1, y1, top] = input.support_box_mm()?;
    let half_x = 0.5 * input.pixel_map().plate_x().value();
    let half_y = 0.5 * input.pixel_map().plate_y().value();
    let past_mm = [-half_x - x0, x1 - half_x, -half_y - y0, y1 - half_y]
        .into_iter()
        .fold(f64::NEG_INFINITY, f64::max);
    let above_mm = top - input.max_z_mm();
    let past = past_mm > options.edge_tolerance_mm;
    if !past && above_mm <= options.height_tolerance_mm {
        return None;
    }
    let focus = PlatePoint::new(0.5 * (x0 + x1), 0.5 * (y0 + y1), input.layers()[0].sample_mm);
    let last = input.layer_count() - 1;
    Some(if past {
        Finding::new(
            CheckKind::PlateEdge,
            Severity::Error,
            0,
            last,
            focus,
            RoleScope::Support,
            "Supports cut by the plate edge",
            format!(
                "The supports reach up to {past_mm:.2} mm past the edge of the plate, and nothing beyond the screen is printed."
            ),
        )
    } else {
        Finding::new(
            CheckKind::TooTall,
            Severity::Error,
            0,
            last,
            focus,
            RoleScope::Support,
            "Supports taller than the printer",
            format!(
                "The supports reach {above_mm:.2} mm above the printer's maximum height, so their top cannot be printed."
            ),
        )
    })
}

#[cfg(test)]
#[path = "tests_output.rs"]
mod tests;
