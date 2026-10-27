/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Output checks (#93): plate edges, maximum height, empty layers and
//! floating parts.
//!
//! Reports [`CheckKind::PlateEdge`], [`CheckKind::TooTall`],
//! [`CheckKind::EmptyLayer`] and [`CheckKind::FloatingPart`] findings, all
//! read from the shared [`PlateRegions`] plus, for the plate edge, the
//! vector contours:
//!
//! - **Plate edge** (error). A printed body (the solid regions joined
//!   through every overlap between layers) with a region on the screen's
//!   outermost pixel row or column: the screen edge cuts it, or it prints on
//!   the very last pixels. One finding per body, from the first to the last
//!   layer it touches the edge, focused on its edge pixel nearest to the
//!   region's centroid. Contours that reach past the plate on a layer and an
//!   edge where no solid touches the screen (a part wholly off the plate, or
//!   a sliver too thin to light a pixel) are reported per leaf instead, so
//!   geometry the raster has lost is still caught; one finding per run of
//!   consecutive layers.
//! - **Too tall** (error). The top of the highest layer with solid is above
//!   the printer's maximum height. One finding, from the first layer with
//!   solid that ends above the limit to the highest one.
//! - **Empty layers**, consecutive ones grouped into one finding. Before the
//!   first solid layer (error: nothing is cured on the plate, so the first
//!   solid layer has nothing to stick to); between solid layers (error: a
//!   gap, so everything above it rests on nothing); after the last solid
//!   layer (info: they cost print time and print nothing).
//! - **Floating part** (error). A printed body whose lowest region is above
//!   layer 0: nothing below it in any earlier layer ever reaches the plate.
//!   A body that starts right above an empty layer is left to the
//!   empty-layer finding, which explains it. It suggests a support under the
//!   body's largest bottom region when that region has model in it.
//!
//! A plate with no solid at all reports only contours outside the plate:
//! without solid there are no layers, heights or parts to judge.

use std::cmp::Reverse;

use rayon::prelude::*;
use sc_sl1cer::contour::Polygon;
use sc_sl1cer::geom2d::Point2D;

use crate::finding::{CheckKind, Finding, PlatePoint, RoleScope, Severity, SuggestedFix};
use crate::input::{ReviewInput, Role};
use crate::regions::{LayerPixels, PixelBox, PlateRegions, Region};
use crate::runner::ReviewContext;
use crate::union_find::{Groups, RegionRef};

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
pub fn run(ctx: &ReviewContext<'_>, options: &OutputOptions) -> Vec<Finding> {
    let mut findings = outside_plate(ctx, options);
    let Some(extent) = SolidExtent::of(ctx.regions) else {
        return findings;
    };
    let bodies = ctx.regions.group_solid(|_, _| true);
    findings.extend(plate_edge(ctx, &bodies));
    findings.extend(too_tall(ctx, extent, options));
    findings.extend(empty_layers(ctx, extent));
    findings.extend(floating_parts(ctx, &bodies));
    findings
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

/// The screen edge at raster column 0.
const EDGE_MIN_X: u8 = 1;
/// The screen edge after the last raster column.
const EDGE_MAX_X: u8 = 2;
/// The screen edge at raster row 0.
const EDGE_MIN_Y: u8 = 4;
/// The screen edge after the last raster row.
const EDGE_MAX_Y: u8 = 8;

/// The screen edges a pixel box lies on, as `EDGE_*` flags.
fn edges_of(bbox: &PixelBox, width_px: u32, height_px: u32) -> u8 {
    let mut edges = 0;
    if bbox.x0 == 0 {
        edges |= EDGE_MIN_X;
    }
    if bbox.x1 == width_px {
        edges |= EDGE_MAX_X;
    }
    if bbox.y0 == 0 {
        edges |= EDGE_MIN_Y;
    }
    if bbox.y1 == height_px {
        edges |= EDGE_MAX_Y;
    }
    edges
}

/// The screen edges some solid of layer `layer` touches, as `EDGE_*` flags.
fn touched_edges(regions: &PlateRegions, layer: usize) -> u8 {
    regions
        .layer(layer)
        .solid
        .iter()
        .filter(|region| region.touches_border)
        .fold(0, |edges, region| {
            edges | edges_of(&region.bbox, regions.width_px(), regions.height_px())
        })
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

// ---------------------------------------------------------------------------
// Plate edge: solid on the outermost pixels.
// ---------------------------------------------------------------------------

/// The part of one body that lies on the screen edge.
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

/// One finding per printed body with solid on the screen's outermost pixels.
fn plate_edge(ctx: &ReviewContext<'_>, bodies: &Groups) -> Vec<Finding> {
    let regions = ctx.regions;
    let contacts: Vec<EdgeContact> = bodies
        .iter()
        .filter_map(|members| EdgeContact::of(regions, members))
        .collect();
    contacts
        .par_iter()
        .map(|contact| {
            let layer = contact.first.layer;
            let pixels = LayerPixels::compute(ctx.input, layer);
            let pixel = edge_pixel(
                &pixels,
                contact.first.index,
                region_of(regions, &contact.first),
                regions.width_px(),
                regions.height_px(),
            );
            let centre = [f64::from(pixel[0]) + 0.5, f64::from(pixel[1]) + 0.5];
            Finding::new(
                CheckKind::PlateEdge,
                Severity::Error,
                layer,
                contact.last_layer,
                ctx.input.focus(layer, centre),
                RoleScope::from_counts(contact.model_px, contact.support_px),
                "Part touches the screen edge",
                format!(
                    "It reaches the outermost pixels of the screen on {}, where the screen ends and anything beyond it is cut off.",
                    plural(contact.layers, "layer", "layers")
                ),
            )
        })
        .collect()
}

/// The pixel of solid region `index` on the screen's outermost row or
/// column that is nearest to the region's centroid, the first in scan order
/// on a tie. Falls back on the region's inside pixel, which cannot happen
/// for a region that touches the edge.
fn edge_pixel(
    pixels: &LayerPixels,
    index: u32,
    region: &Region,
    width_px: u32,
    height_px: u32,
) -> [u32; 2] {
    let [cx, cy] = region.centroid_px;
    let mut best = (f64::INFINITY, region.inside_px);
    let mut consider = |x: u32, y: u32| {
        let dx = f64::from(x) + 0.5 - cx;
        let dy = f64::from(y) + 0.5 - cy;
        let distance = dx.mul_add(dx, dy * dy);
        if distance < best.0 {
            best = (distance, [x, y]);
        }
    };
    for (run, &label) in pixels.solid.iter().zip(&pixels.solid_labels) {
        if label != index {
            continue;
        }
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
    best.1
}

// ---------------------------------------------------------------------------
// Plate edge: contours past the plate that the raster has lost.
// ---------------------------------------------------------------------------

/// How far one leaf's contours reach past one screen edge on one layer.
#[derive(Clone, Copy, Debug)]
struct Overhang {
    /// Index of the leaf in [`ReviewInput::leaves`].
    leaf: usize,
    /// The screen edge, an `EDGE_*` flag.
    edge: u8,
    /// Millimetres past the edge.
    past_mm: f64,
    /// The vertex that reaches farthest past it, in plate millimetres.
    vertex: Point2D,
}

/// The vertices of `polygons` reaching farthest towards each side of the
/// plate: `[min x, max x, min y, max y]` in plate millimetres. Holes lie
/// inside their outer ring, so only outer rings are read.
fn extremes(polygons: &[Polygon]) -> Option<[Point2D; 4]> {
    let mut vertices = polygons
        .iter()
        .flat_map(|polygon| polygon.outer().vertices().iter().copied());
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

/// Every leaf's contours past the plate on layer `layer`, by screen edge.
fn overhangs(input: &ReviewInput<'_>, layer: usize, tolerance_mm: f64) -> Vec<Overhang> {
    let half_x = 0.5 * input.pixel_map().plate_x().value();
    let half_y = 0.5 * input.pixel_map().plate_y().value();
    let mirror = input.pixel_map().mirror();
    // The plate's -x, +x, -y and +y sides as screen edges: a mirrored axis
    // swaps them.
    let (low_x, high_x) = if mirror.x {
        (EDGE_MAX_X, EDGE_MIN_X)
    } else {
        (EDGE_MIN_X, EDGE_MAX_X)
    };
    let (low_y, high_y) = if mirror.y {
        (EDGE_MAX_Y, EDGE_MIN_Y)
    } else {
        (EDGE_MIN_Y, EDGE_MAX_Y)
    };
    let mut found = Vec::new();
    for (leaf, review_leaf) in input.leaves().iter().enumerate() {
        let Some([min_x, max_x, min_y, max_y]) =
            extremes(review_leaf.stack.layers()[layer].polygons())
        else {
            continue;
        };
        for (edge, past_mm, vertex) in [
            (low_x, -half_x - min_x.x, min_x),
            (high_x, max_x.x - half_x, max_x),
            (low_y, -half_y - min_y.y, min_y),
            (high_y, max_y.y - half_y, max_y),
        ] {
            if past_mm > tolerance_mm {
                found.push(Overhang {
                    leaf,
                    edge,
                    past_mm,
                    vertex,
                });
            }
        }
    }
    found
}

/// Consecutive layers on which one leaf reaches past the plate.
struct OutsideRun {
    first: usize,
    last: usize,
    /// The farthest past the plate on any of its layers.
    past_mm: f64,
    /// The farthest past the plate on its first layer, and where.
    first_past_mm: f64,
    focus: PlatePoint,
}

/// One finding per leaf and run of consecutive layers whose contours reach
/// past an edge of the plate where no solid touches the screen on that
/// layer. Where solid does touch that edge, the body finding reports it.
fn outside_plate(ctx: &ReviewContext<'_>, options: &OutputOptions) -> Vec<Finding> {
    let input = ctx.input;
    let per_layer: Vec<Vec<Overhang>> = (0..input.layer_count())
        .into_par_iter()
        .map(|layer| {
            let mut found = overhangs(input, layer, options.edge_tolerance_mm);
            if !found.is_empty() {
                let touched = touched_edges(ctx.regions, layer);
                found.retain(|overhang| overhang.edge & touched == 0);
            }
            found
        })
        .collect();
    let mut runs: Vec<Vec<OutsideRun>> = input.leaves().iter().map(|_| Vec::new()).collect();
    for (layer, found) in per_layer.iter().enumerate() {
        for overhang in found {
            let focus = PlatePoint::new(
                overhang.vertex.x,
                overhang.vertex.y,
                input.layers()[layer].sample_mm,
            );
            let leaf_runs = &mut runs[overhang.leaf];
            match leaf_runs.last_mut() {
                Some(run) if run.last + 1 >= layer => {
                    run.last = layer;
                    run.past_mm = run.past_mm.max(overhang.past_mm);
                    if run.first == layer && overhang.past_mm > run.first_past_mm {
                        run.first_past_mm = overhang.past_mm;
                        run.focus = focus;
                    }
                }
                _ => leaf_runs.push(OutsideRun {
                    first: layer,
                    last: layer,
                    past_mm: overhang.past_mm,
                    first_past_mm: overhang.past_mm,
                    focus,
                }),
            }
        }
    }
    let mut findings = Vec::new();
    for (leaf, leaf_runs) in input.leaves().iter().zip(runs) {
        let who = match (leaf.name, leaf.role) {
            (Some(name), _) => format!("'{name}'"),
            (None, Role::Model) => "A model".to_string(),
            (None, Role::Support) => "A support".to_string(),
        };
        let scope = match leaf.role {
            Role::Model => RoleScope::Model,
            Role::Support => RoleScope::Support,
        };
        for run in leaf_runs {
            findings.push(Finding::new(
                CheckKind::PlateEdge,
                Severity::Error,
                run.first,
                run.last,
                run.focus,
                scope,
                "Part outside the printable area",
                format!(
                    "{who} reaches up to {:.2} mm past the edge of the plate on {}, and nothing beyond the screen is printed.",
                    run.past_mm,
                    plural(run.last - run.first + 1, "layer", "layers")
                ),
            ));
        }
    }
    findings
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
                "{} a {gap_mm:.2} mm gap, so everything above it has nothing to rest on.",
                plural(layer - start, "empty layer leaves", "empty layers leave")
            ),
        ));
    }
    let count = input.layer_count() - 1 - extent.last;
    if count > 0 {
        let first = extent.last + 1;
        let height_mm = layers[input.layer_count() - 1].top_mm - layers[first].bottom_mm;
        findings.push(Finding::new(
            CheckKind::EmptyLayer,
            Severity::Info,
            first,
            input.layer_count() - 1,
            focus(first, extent.last),
            RoleScope::Both,
            if count == 1 {
                "Empty layer at the top"
            } else {
                "Empty layers at the top"
            },
            format!(
                "The last {} empty ({height_mm:.2} mm), which adds print time and prints nothing.",
                counted(count, "layer is", "layers are")
            ),
        ));
    }
    findings
}

// ---------------------------------------------------------------------------
// Floating parts.
// ---------------------------------------------------------------------------

/// One finding per printed body that never reaches the plate, unless it
/// starts right above an empty layer, which the empty-layer finding
/// explains.
fn floating_parts(ctx: &ReviewContext<'_>, bodies: &Groups) -> Vec<Finding> {
    let (input, regions) = (ctx.input, ctx.regions);
    let layers = input.layers();
    let pixel_mm2 = input.pixel_area_mm2();
    let mut findings = Vec::new();
    for members in bodies.iter() {
        let bottom = members[0].layer;
        if bottom == 0 || regions.layer(bottom - 1).is_empty() {
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
        let finding = Finding::new(
            CheckKind::FloatingPart,
            Severity::Error,
            bottom,
            members[members.len() - 1].layer,
            focus,
            RoleScope::from_counts(model_px, support_px),
            "Floating part",
            format!(
                "This {} mm³ part starts {:.2} mm above the build plate and never joins anything standing on the plate, so it will not stay attached.",
                volume_text(volume_mm3),
                layers[bottom].bottom_mm
            ),
        );
        findings.push(if base.model_px > 0 {
            finding.with_fix(SuggestedFix::AddSupport { at: focus })
        } else {
            finding
        });
    }
    findings
}

#[cfg(test)]
#[path = "tests_output.rs"]
mod tests;
