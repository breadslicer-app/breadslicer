//! DragonFruit `geometry/computeRaftFootprint.ts`: the raft footprint as a
//! polygon set — the convex hull of the support roots (per cluster the model
//! clearance separates), minus the models' plate footprint grown by the
//! clearance — and the raft height helpers.

use std::collections::HashMap;

use super::super::raft_types::{RaftBottomMode, RaftSettings, SupportBaseCircle};
use super::model_plate_footprint::MODEL_PLATE_CLEARANCE_MM;
use super::polygon_set_2d::{
    EDGE_QUERY_EPS_MM, PolygonWithHoles, build_clearance_edge_grid, difference_polygon_sets,
    grid_segment_blocked, offset_polygon_set, polygon_set_contains, ring_to_polygon,
};
use super::small::compute_footprint;
use crate::clipper::ClipperError;
use crate::three::math_utils::{js_max, js_min, js_tan};

/// Material height of the raft body: beam height in line mode, plate
/// thickness otherwise.
pub fn raft_solid_height_mm(raft: &RaftSettings) -> f64 {
    if raft.bottom_mode == RaftBottomMode::Line {
        js_max(0.0, raft.line_height_mm)
    } else {
        js_max(0.0, raft.thickness)
    }
}

pub fn raft_chamfer_inset_mm(raft: &RaftSettings) -> f64 {
    let angle = js_min(90.0, js_max(45.0, raft.chamfer_angle));
    raft_solid_height_mm(raft) * js_tan((std::f64::consts::PI / 180.0) * (90.0 - angle))
}

/// Hull margin: clears the chamfer inset and the wall.
pub fn raft_dynamic_margin_mm(raft: &RaftSettings) -> f64 {
    let wall_inset = if raft.wall_enabled {
        js_max(0.0, raft.wall_thickness)
    } else {
        0.0
    };
    0.2 + js_max(raft_chamfer_inset_mm(raft), wall_inset)
}

/// Top of the material a model has to clear: plate/beam plus the wall.
pub fn raft_band_top_mm(raft: &RaftSettings) -> f64 {
    raft_solid_height_mm(raft)
        + if raft.wall_enabled {
            js_max(0.0, raft.wall_height)
        } else {
            0.0
        }
}

/// Z the wall is extruded from.
pub fn raft_wall_base_height_mm(raft: &RaftSettings) -> f64 {
    if raft.bottom_mode == RaftBottomMode::Line {
        js_max(0.01, raft.line_height_mm)
    } else {
        js_max(0.0, raft.thickness)
    }
}

/// The region a raft must stay out of: the plate footprint grown by the
/// clearance.
pub fn inflate_model_plate_clearance(
    clearance: &[PolygonWithHoles],
) -> Result<Vec<PolygonWithHoles>, ClipperError> {
    if clearance.is_empty() {
        return Ok(Vec::new());
    }
    offset_polygon_set(clearance, MODEL_PLATE_CLEARANCE_MM)
}

/// `clusterCirclesAroundClearance`: two roots share a raft only if the line
/// between them clears the model.
fn cluster_circles_around_clearance(
    circles: &[SupportBaseCircle],
    clearance: &[PolygonWithHoles],
) -> Vec<Vec<SupportBaseCircle>> {
    if circles.len() <= 1 {
        return vec![circles.to_vec()];
    }
    let Some(grid) = build_clearance_edge_grid(clearance, 4.0) else {
        return vec![circles.to_vec()];
    };
    let inside: Vec<bool> = circles
        .iter()
        .map(|c| polygon_set_contains(clearance, c.x, c.y))
        .collect();
    let mut parent: Vec<usize> = (0..circles.len()).collect();
    let find = |parent: &Vec<usize>, index: usize| {
        let mut root = index;
        while parent[root] != root {
            root = parent[root];
        }
        root
    };
    for i in 0..circles.len() {
        for j in i + 1..circles.len() {
            if inside[i] || inside[j] {
                continue;
            }
            let (a, b) = (circles[i], circles[j]);
            if !grid_segment_blocked(&grid, a.x, a.y, b.x, b.y, EDGE_QUERY_EPS_MM) {
                let (ra, rb) = (find(&parent, i), find(&parent, j));
                if ra != rb {
                    parent[rb] = ra;
                }
            }
        }
    }
    // A Map keyed by root: clusters in order of their first member.
    let mut order: Vec<usize> = Vec::new();
    let mut clusters: HashMap<usize, Vec<SupportBaseCircle>> = HashMap::new();
    for (i, c) in circles.iter().enumerate() {
        let root = find(&parent, i);
        clusters
            .entry(root)
            .or_insert_with(|| {
                order.push(root);
                Vec::new()
            })
            .push(*c);
    }
    order
        .into_iter()
        .map(|r| clusters.remove(&r).unwrap())
        .collect()
}

/// `computeRaftFootprintPolygons({ circles, raft, clearance, samplesPerCircle })`.
pub fn compute_raft_footprint_polygons(
    circles: &[SupportBaseCircle],
    raft: &RaftSettings,
    clearance: Option<&[PolygonWithHoles]>,
    samples_per_circle: Option<f64>,
) -> Result<Vec<PolygonWithHoles>, ClipperError> {
    if circles.is_empty() {
        return Ok(Vec::new());
    }
    let margin_mm = raft_dynamic_margin_mm(raft);
    let samples = samples_per_circle.unwrap_or(24.0);
    let inflated = match clearance {
        Some(c) if !c.is_empty() => inflate_model_plate_clearance(c)?,
        _ => Vec::new(),
    };
    let clusters = if !inflated.is_empty() {
        cluster_circles_around_clearance(circles, &inflated)
    } else {
        vec![circles.to_vec()]
    };
    let mut footprint = Vec::new();
    for cluster in &clusters {
        let hull = compute_footprint(cluster, Some(margin_mm), Some(samples));
        if hull.len() >= 3 {
            footprint.push(ring_to_polygon(hull));
        }
    }
    if footprint.is_empty() {
        return Ok(Vec::new());
    }
    if inflated.is_empty() {
        return Ok(footprint);
    }
    difference_polygon_sets(&footprint, &inflated)
}
