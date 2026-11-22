//! A raft's outline: the hull of the support roots, one per group of roots
//! the model's footprint does not separate, with the footprint (grown by its
//! clearance) cut out; and the raft's heights.

use super::super::raft_types::{RaftBottomMode, RaftSettings, SupportBaseCircle};
use super::model_plate_footprint::MODEL_PLATE_CLEARANCE_MM;
use super::polygon_set_2d::{
    EDGE_QUERY_EPS_MM, PolygonError, PolygonWithHoles, build_clearance_edge_grid,
    difference_polygon_sets, grid_segment_blocked, offset_polygon_set, polygon_set_contains,
    ring_to_polygon,
};
use super::raft_body::compute_footprint;

/// The raft's own body: the beams' height for lines, the plate's otherwise.
pub fn raft_solid_height_mm(raft: &RaftSettings) -> f64 {
    let height = if raft.bottom_mode == RaftBottomMode::Line {
        raft.line_height_mm
    } else {
        raft.thickness
    };
    height.max(0.0)
}

/// How far the chamfer slopes the raft's sides in over its body.
pub fn raft_chamfer_inset_mm(raft: &RaftSettings) -> f64 {
    let angle = raft.chamfer_angle.clamp(45.0, 90.0);
    raft_solid_height_mm(raft) * (90.0 - angle).to_radians().tan()
}

/// How far the hull reaches past the roots: past the chamfer and the wall,
/// with 0.2 mm to spare.
pub fn raft_dynamic_margin_mm(raft: &RaftSettings) -> f64 {
    let wall = if raft.wall_enabled {
        raft.wall_thickness.max(0.0)
    } else {
        0.0
    };
    0.2 + raft_chamfer_inset_mm(raft).max(wall)
}

/// The top of the raft a model has to clear: its body and its wall.
pub fn raft_band_top_mm(raft: &RaftSettings) -> f64 {
    let wall = if raft.wall_enabled {
        raft.wall_height.max(0.0)
    } else {
        0.0
    };
    raft_solid_height_mm(raft) + wall
}

/// Where the wall stands: on the beams (at least 0.01 mm) or the plate.
pub fn raft_wall_base_height_mm(raft: &RaftSettings) -> f64 {
    if raft.bottom_mode == RaftBottomMode::Line {
        raft.line_height_mm.max(0.01)
    } else {
        raft.thickness.max(0.0)
    }
}

/// What a raft stays out of: the models' footprint grown by the clearance.
pub fn inflate_model_plate_clearance(
    clearance: &[PolygonWithHoles],
) -> Result<Vec<PolygonWithHoles>, PolygonError> {
    if clearance.is_empty() {
        return Ok(Vec::new());
    }
    offset_polygon_set(clearance, MODEL_PLATE_CLEARANCE_MM)
}

/// The roots in groups: two roots share a raft when neither stands in the
/// clearance and the line between them crosses none of it. Groups come in
/// the order of their first root.
fn grouped_roots(
    circles: &[SupportBaseCircle],
    clearance: &[PolygonWithHoles],
) -> Vec<Vec<SupportBaseCircle>> {
    if circles.len() <= 1 {
        return vec![circles.to_vec()];
    }
    let Some(edges) = build_clearance_edge_grid(clearance, 4.0) else {
        return vec![circles.to_vec()];
    };
    let blocked: Vec<bool> = circles
        .iter()
        .map(|c| polygon_set_contains(clearance, c.x, c.y))
        .collect();
    let mut group: Vec<usize> = (0..circles.len()).collect();
    let root = |group: &[usize], mut i: usize| {
        while group[i] != i {
            i = group[i];
        }
        i
    };
    for i in 0..circles.len() {
        for j in (i + 1)..circles.len() {
            let (a, b) = (circles[i], circles[j]);
            if blocked[i]
                || blocked[j]
                || grid_segment_blocked(&edges, a.x, a.y, b.x, b.y, EDGE_QUERY_EPS_MM)
            {
                continue;
            }
            let (ri, rj) = (root(&group, i), root(&group, j));
            if ri != rj {
                group[rj] = ri;
            }
        }
    }
    let mut firsts: Vec<usize> = Vec::new();
    let mut out: Vec<Vec<SupportBaseCircle>> = Vec::new();
    for (i, c) in circles.iter().enumerate() {
        let r = root(&group, i);
        match firsts.iter().position(|&f| f == r) {
            Some(k) => out[k].push(*c),
            None => {
                firsts.push(r);
                out.push(vec![*c]);
            }
        }
    }
    out
}

/// The raft's outline over `circles` (the support roots), each group's hull
/// sampled at `samples_per_circle` points a root (24 by default), with the
/// models' clearance cut out.
pub fn compute_raft_footprint_polygons(
    circles: &[SupportBaseCircle],
    raft: &RaftSettings,
    clearance: Option<&[PolygonWithHoles]>,
    samples_per_circle: Option<f64>,
) -> Result<Vec<PolygonWithHoles>, PolygonError> {
    if circles.is_empty() {
        return Ok(Vec::new());
    }
    let margin = raft_dynamic_margin_mm(raft);
    let samples = samples_per_circle.unwrap_or(24.0);
    let keep_out = match clearance {
        Some(c) if !c.is_empty() => inflate_model_plate_clearance(c)?,
        _ => Vec::new(),
    };
    let groups = if keep_out.is_empty() {
        vec![circles.to_vec()]
    } else {
        grouped_roots(circles, &keep_out)
    };
    let outline: Vec<PolygonWithHoles> = groups
        .iter()
        .map(|g| compute_footprint(g, Some(margin), Some(samples)))
        .filter(|hull| hull.len() >= 3)
        .map(ring_to_polygon)
        .collect();
    if outline.is_empty() || keep_out.is_empty() {
        return Ok(outline);
    }
    difference_polygon_sets(&outline, &keep_out)
}
