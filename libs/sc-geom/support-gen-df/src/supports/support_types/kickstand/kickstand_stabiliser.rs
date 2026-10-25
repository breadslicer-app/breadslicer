//! DragonFruit `src/supports/SupportTypes/Kickstand/kickstandStabiliser.ts`:
//! the kickstands auto-bracing generates for a tall trunk whose brace group
//! lacks two bracing axes — dropped parallel to the trunk, snapped to the
//! grid's cardinal neighbours when it is on, clear of roots, shafts and the
//! model, the snuggest legal drop per axis, then a cleanup pass for groups
//! still short of two axes.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use super::kickstand_builder::{KickstandBuildInput, build_kickstand_data};
use super::types::{KickstandBuildResult, KickstandHostTarget};
use crate::supports::auto_bracing::auto_brace::store_type_of;
use crate::supports::auto_bracing::mesh_clearance::{BraceMeshes, line_passes_mesh_clearance};
use crate::supports::auto_bracing::settings::{AutoBracingSettings, auto_bracing_hard_rules};
use crate::supports::auto_bracing::two_axis_detection::{
    additional_axes_needed_for_two_axis_bracing, has_qualified_two_axis_bracing,
    normalize_axis_angle_rad,
};
use crate::supports::placement_logic::grid::grid_math::snap_to_grid_index;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::knot::segment_endpoints::{
    EndpointHosts, ShaftEntity, resolve_segment_endpoints,
};
use crate::supports::support_type_registry::is_kickstand_host_type;
use crate::supports::types::{Kickstand, Knot, Roots, SupportState, Vec3};
use crate::three::math_utils::{js_atan2, js_cos, js_max, js_min, js_sin};
use crate::volume_analysis::islands::voxel_footprint::number_key;

const MIN_HEIGHT_FOR_MANDATORY_BRACING_MM: f64 = 15.0;
const DROP_COLLISION_SAMPLES: usize = 20;
const MAX_GRID_ROOT_SEARCH_RING: f64 = 1.0;
const MIN_ROOT_PROXIMITY_CLEARANCE_MM: f64 = 2.0;

/// The stabiliser slice of the state a regeneration pass works on.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StabiliserState {
    pub kickstands: IndexMap<String, Kickstand>,
    pub roots: IndexMap<String, Roots>,
    pub knots: IndexMap<String, Knot>,
}

/// A trunk-to-trunk brace axis the preliminary pairing found.
#[derive(Debug, Clone, PartialEq)]
pub struct BraceAxisEdge {
    pub a: String,
    pub b: String,
    pub angle_rad: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSnap {
    pub enabled: bool,
    pub spacing_mm: f64,
}

struct CandidateAnchor {
    segment_id: String,
    t: f64,
    pos: Vec3,
    diameter_mm: f64,
}

/// `${modelId}:${gx},${gy}`.
fn grid_node_key(model_id: &str, gx: f64, gy: f64) -> (String, u64, u64) {
    (model_id.to_owned(), number_key(gx), number_key(gy))
}

/// `generateRequiredKickstands`. `live` is `getSettings()`, which the
/// kickstand builder reads, and `store` the committed store that resolves an
/// unstamped trunk's type; trial builds draw ids whether or not they are kept.
#[allow(clippy::too_many_arguments)]
pub fn generate_required_kickstands(
    snapshot: &SupportState,
    kickstand_state: &StabiliserState,
    settings: &AutoBracingSettings,
    existing_brace_edges: &[BraceAxisEdge],
    grid: GridSnap,
    meshes: &BraceMeshes<'_>,
    live: &SupportSettings,
    store: &SupportState,
    new_id: &mut dyn FnMut() -> String,
) -> Vec<KickstandBuildResult> {
    let c = &snapshot.collections;
    let mut generated: Vec<KickstandBuildResult> = Vec::new();
    let max_horizontal_run = settings.max_brace_length_mm;
    let generation_distance_mm = js_min(5.0, max_horizontal_run * 0.8);

    let mut occupied_root_positions: Vec<Vec3> = c
        .roots
        .values()
        .chain(kickstand_state.roots.values())
        .map(|r| r.transform.pos)
        .collect();
    let min_clear_sq = MIN_ROOT_PROXIMITY_CLEARANCE_MM * MIN_ROOT_PROXIMITY_CLEARANCE_MM;
    let too_close_to_root = |occupied: &[Vec3], x: f64, y: f64| {
        occupied.iter().any(|r| {
            let dx = r.x - x;
            let dy = r.y - y;
            dx * dx + dy * dy <= min_clear_sq
        })
    };

    let is_grid_enabled = grid.enabled && grid.spacing_mm > 0.0;
    let mut occupied_grid_nodes: HashSet<(String, u64, u64)> = HashSet::new();
    if is_grid_enabled {
        for root in c.roots.values().chain(kickstand_state.roots.values()) {
            occupied_grid_nodes.insert(grid_node_key(
                &root.base.model_id,
                snap_to_grid_index(root.transform.pos.x, grid.spacing_mm),
                snap_to_grid_index(root.transform.pos.y, grid.spacing_mm),
            ));
        }
    }

    let mut segment_owner_trunk_id: HashMap<&str, &str> = HashMap::new();
    for trunk in c.trunks.values() {
        for seg in &trunk.segments {
            segment_owner_trunk_id.insert(seg.id.as_str(), trunk.base.id.as_str());
        }
    }

    let mut axes_by_trunk_id: HashMap<String, Vec<f64>> = HashMap::new();
    for edge in existing_brace_edges {
        axes_by_trunk_id
            .entry(edge.a.clone())
            .or_default()
            .push(edge.angle_rad);
        axes_by_trunk_id
            .entry(edge.b.clone())
            .or_default()
            .push(edge.angle_rad);
    }
    for sb in kickstand_state.kickstands.values() {
        let Some(host_trunk_id) = segment_owner_trunk_id.get(sb.host_segment_id.as_str()) else {
            continue;
        };
        let (Some(root), Some(host_knot)) = (
            kickstand_state.roots.get(&sb.root_id),
            kickstand_state.knots.get(&sb.host_knot_id),
        ) else {
            continue;
        };
        let angle = normalize_axis_angle_rad(js_atan2(
            root.transform.pos.y - host_knot.pos.y,
            root.transform.pos.x - host_knot.pos.x,
        ));
        axes_by_trunk_id
            .entry((*host_trunk_id).to_owned())
            .or_default()
            .push(angle);
    }

    // Brace groups over trunk-to-trunk edges, and each group's axes.
    let mut trunk_adjacency: IndexMap<&str, indexmap::IndexSet<&str>> = c
        .trunks
        .keys()
        .map(|id| (id.as_str(), indexmap::IndexSet::new()))
        .collect();
    for edge in existing_brace_edges {
        if !trunk_adjacency.contains_key(edge.a.as_str())
            || !trunk_adjacency.contains_key(edge.b.as_str())
        {
            continue;
        }
        trunk_adjacency
            .get_mut(edge.a.as_str())
            .expect("present")
            .insert(edge.b.as_str());
        trunk_adjacency
            .get_mut(edge.b.as_str())
            .expect("present")
            .insert(edge.a.as_str());
    }
    let mut visited: HashSet<&str> = HashSet::new();
    let mut group_id_by_trunk_id: HashMap<&str, String> = HashMap::new();
    let mut group_axes_by_group_id: HashMap<String, Vec<f64>> = HashMap::new();
    let mut remaining_need_by_group_id: HashMap<String, u8> = HashMap::new();
    for trunk_id in c.trunks.keys() {
        if !visited.insert(trunk_id.as_str()) {
            continue;
        }
        let mut members: Vec<&str> = Vec::new();
        let mut queue: Vec<&str> = vec![trunk_id.as_str()];
        let mut cursor = 0;
        while cursor < queue.len() {
            let current = queue[cursor];
            cursor += 1;
            members.push(current);
            if let Some(neighbors) = trunk_adjacency.get(current) {
                for n in neighbors {
                    if visited.insert(n) {
                        queue.push(n);
                    }
                }
            }
        }
        let member_set: HashSet<&str> = members.iter().copied().collect();
        let group_axes: Vec<f64> = existing_brace_edges
            .iter()
            .filter(|e| member_set.contains(e.a.as_str()) && member_set.contains(e.b.as_str()))
            .map(|e| e.angle_rad)
            .collect();
        let group_id = members[0].to_owned();
        remaining_need_by_group_id.insert(
            group_id.clone(),
            additional_axes_needed_for_two_axis_bracing(
                &group_axes,
                auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG,
            ),
        );
        group_axes_by_group_id.insert(group_id.clone(), group_axes);
        for m in members {
            group_id_by_trunk_id.insert(m, group_id.clone());
        }
    }

    let check_slanted_collision = |top: Vec3, bottom: Vec3, model_id: &str, radius: f64| -> bool {
        let Some(entry) = meshes.get(model_id) else {
            return false;
        };
        let inverse = entry.matrix.invert();
        let start = top.apply_matrix4(&inverse);
        let end = bottom.apply_matrix4(&inverse);
        for i in 0..=DROP_COLLISION_SAMPLES {
            let t = i as f64 / DROP_COLLISION_SAMPLES as f64;
            let p = start.lerp(end, t);
            if entry
                .bvh
                .closest_point_to_point(p, 0.0, f64::INFINITY)
                .is_some_and(|r| r.distance < radius)
            {
                return true;
            }
        }
        false
    };
    let built_passes_clearance = |build: &KickstandBuildResult, model_id: &str| -> bool {
        let body = js_max(0.001, build.kickstand.profile.body_diameter_mm);
        let root_pos = build.root.transform.pos;
        let mut path = vec![Vec3::new(
            root_pos.x,
            root_pos.y,
            root_pos.z + build.root.disk_height + build.root.cone_height,
        )];
        for seg in &build.kickstand.segments {
            if let Some(j) = &seg.bottom_joint {
                path.push(j.pos);
            }
            if let Some(j) = &seg.top_joint {
                path.push(j.pos);
            }
        }
        path.push(build.host_knot.pos);
        let mesh = meshes.get(model_id);
        path.windows(2).all(|w| {
            let (a, b) = (w[0], w[1]);
            let (dx, dy, dz) = (b.x - a.x, b.y - a.y, b.z - a.z);
            dx * dx + dy * dy + dz * dz < 0.000001 || line_passes_mesh_clearance(a, b, mesh, body)
        })
    };

    for pass_index in 0..2 {
        let is_cleanup_pass = pass_index == 1;
        for trunk in c.trunks.values() {
            let Some(root) = c.roots.get(&trunk.root_id) else {
                continue;
            };
            // The trunk's own stamped type, when a kickstand may host on it.
            let Some(host_kind) = trunk
                .base
                .type_id
                .or_else(|| store_type_of(&store.collections, &trunk.base.id))
                .filter(|t| is_kickstand_host_type(t.as_str()))
            else {
                continue;
            };

            let mut max_z = root.transform.pos.z;
            let mut candidate_anchors: Vec<CandidateAnchor> = Vec::new();
            for (idx, seg) in trunk.segments.iter().enumerate() {
                let Some(ep) = resolve_segment_endpoints(
                    ShaftEntity::Trunk(trunk),
                    seg,
                    idx,
                    EndpointHosts {
                        root: Some(root),
                        host_knot: None,
                    },
                ) else {
                    continue;
                };
                if ep.end.z > max_z {
                    max_z = ep.end.z;
                }
                let mut t = 0.9;
                while t >= 0.1 {
                    candidate_anchors.push(CandidateAnchor {
                        segment_id: seg.id.clone(),
                        t,
                        pos: Vec3::new(
                            ep.start.x + (ep.end.x - ep.start.x) * t,
                            ep.start.y + (ep.end.y - ep.start.y) * t,
                            ep.start.z + (ep.end.z - ep.start.z) * t,
                        ),
                        diameter_mm: seg.diameter,
                    });
                    t -= 0.2;
                }
            }
            if max_z - root.transform.pos.z < MIN_HEIGHT_FOR_MANDATORY_BRACING_MM {
                continue;
            }

            let trunk_id = trunk.base.id.as_str();
            let local_axes = axes_by_trunk_id.get(trunk_id).cloned().unwrap_or_default();
            let group_id = group_id_by_trunk_id
                .get(trunk_id)
                .cloned()
                .unwrap_or_else(|| trunk_id.to_owned());
            let decision_axes = group_axes_by_group_id
                .get(&group_id)
                .cloned()
                .unwrap_or_default();
            let has_two_axis = has_qualified_two_axis_bracing(
                &decision_axes,
                auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG,
            );
            let mut remaining_group_need = remaining_need_by_group_id
                .get(&group_id)
                .copied()
                .unwrap_or(0);
            if has_two_axis || remaining_group_need == 0 {
                continue;
            }
            let max_braces_to_generate = remaining_group_need.clamp(1, 2);
            if candidate_anchors.is_empty() {
                continue;
            }
            // Highest first, so the kickstand attaches as high as it can.
            candidate_anchors.sort_by(|a, b| {
                crate::supports::auto_support::types::js_compare(b.pos.z - a.pos.z)
            });

            let root_pos = root.transform.pos;
            let mut existing_axis = decision_axes
                .first()
                .copied()
                .or_else(|| local_axes.first().copied())
                .unwrap_or(0.0);
            let mut generated_axes = decision_axes.clone();
            let mut used_root_positions: Vec<Vec3> = Vec::new();

            for i in 0..max_braces_to_generate {
                if has_qualified_two_axis_bracing(
                    &generated_axes,
                    auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG,
                ) || remaining_group_need == 0
                {
                    break;
                }
                let angle_offset = if max_braces_to_generate == 2 {
                    if i == 0 {
                        0.0
                    } else {
                        existing_axis + std::f64::consts::PI / 2.0
                    }
                } else {
                    existing_axis + std::f64::consts::PI / 2.0
                };
                let drop_dist = generation_distance_mm;
                let mut selected: Option<(KickstandBuildResult, Vec3, f64)> = None;

                for anchor in &candidate_anchors {
                    let trunk_dx = anchor.pos.x - root_pos.x;
                    let trunk_dy = anchor.pos.y - root_pos.y;
                    let max_attempts = if is_cleanup_pass { 12 } else { 8 };
                    for attempt in 0..max_attempts {
                        let mut angle = angle_offset
                            + if attempt % 2 == 0 {
                                0.0
                            } else {
                                std::f64::consts::PI
                            };
                        if is_cleanup_pass {
                            match attempt % 6 {
                                2 => angle = existing_axis + std::f64::consts::PI / 2.0,
                                3 => angle = existing_axis + (3.0 * std::f64::consts::PI) / 2.0,
                                4 => angle = existing_axis,
                                5 => angle = existing_axis + std::f64::consts::PI,
                                _ => {}
                            }
                        }
                        let current_dist = if is_grid_enabled {
                            drop_dist
                        } else {
                            drop_dist + (attempt / 2) as f64 * 2.0
                        };
                        if current_dist > max_horizontal_run {
                            break;
                        }
                        let top_anchor_x = anchor.pos.x + js_cos(angle) * current_dist;
                        let top_anchor_y = anchor.pos.y + js_sin(angle) * current_dist;
                        let mut root_x = top_anchor_x - trunk_dx;
                        let mut root_y = top_anchor_y - trunk_dy;

                        if is_grid_enabled {
                            let spacing = grid.spacing_mm;
                            let center_gx = snap_to_grid_index(root_pos.x, spacing);
                            let center_gy = snap_to_grid_index(root_pos.y, spacing);
                            let default_max_ring =
                                js_max(1.0, (max_horizontal_run / spacing).ceil() + 2.0);
                            let max_ring =
                                js_max(0.0, js_min(default_max_ring, MAX_GRID_ROOT_SEARCH_RING));
                            if max_ring < 1.0 {
                                continue;
                            }
                            let mut best: Option<(f64, f64, f64)> = None;
                            for (gx, gy) in [
                                (center_gx + 1.0, center_gy),
                                (center_gx - 1.0, center_gy),
                                (center_gx, center_gy + 1.0),
                                (center_gx, center_gy - 1.0),
                            ] {
                                let wx = gx * spacing;
                                let wy = gy * spacing;
                                if occupied_grid_nodes.contains(&grid_node_key(
                                    &trunk.base.model_id,
                                    gx,
                                    gy,
                                )) {
                                    continue;
                                }
                                if too_close_to_root(&occupied_root_positions, wx, wy) {
                                    continue;
                                }
                                let (ddx, ddy) = (wx - root_x, wy - root_y);
                                let dist_sq = ddx * ddx + ddy * ddy;
                                if best.is_none_or(|b| dist_sq < b.2) {
                                    best = Some((wx, wy, dist_sq));
                                }
                            }
                            let Some((sx, sy, _)) = best else {
                                continue;
                            };
                            root_x = snap_to_grid_index(sx, spacing) * spacing;
                            root_y = snap_to_grid_index(sy, spacing) * spacing;
                            let host_gx = snap_to_grid_index(root_pos.x, spacing);
                            let host_gy = snap_to_grid_index(root_pos.y, spacing);
                            let cand_gx = snap_to_grid_index(root_x, spacing);
                            let cand_gy = snap_to_grid_index(root_y, spacing);
                            if (cand_gx - host_gx).abs() > 1.0 || (cand_gy - host_gy).abs() > 1.0 {
                                continue;
                            }
                        }

                        let candidate_root_pos = Vec3::new(root_x, root_y, 0.0);
                        let (hdx, hdy) = (root_x - anchor.pos.x, root_y - anchor.pos.y);
                        if hdx * hdx + hdy * hdy <= min_clear_sq {
                            continue;
                        }
                        let (rdx, rdy) = (root_x - root_pos.x, root_y - root_pos.y);
                        if rdx * rdx + rdy * rdy <= min_clear_sq {
                            continue;
                        }
                        let seg_dx = anchor.pos.x - root_pos.x;
                        let seg_dy = anchor.pos.y - root_pos.y;
                        let seg_len_sq = seg_dx * seg_dx + seg_dy * seg_dy;
                        if seg_len_sq > 0.000001 {
                            let t = js_max(
                                0.0,
                                js_min(
                                    1.0,
                                    ((root_x - root_pos.x) * seg_dx
                                        + (root_y - root_pos.y) * seg_dy)
                                        / seg_len_sq,
                                ),
                            );
                            let sdx = root_x - (root_pos.x + seg_dx * t);
                            let sdy = root_y - (root_pos.y + seg_dy * t);
                            if sdx * sdx + sdy * sdy <= min_clear_sq {
                                continue;
                            }
                        } else if rdx * rdx + rdy * rdy <= min_clear_sq {
                            continue;
                        }
                        if too_close_to_root(&occupied_root_positions, root_x, root_y) {
                            continue;
                        }
                        if used_root_positions
                            .iter()
                            .any(|u| (u.x - root_x).abs() < 0.1 && (u.y - root_y).abs() < 0.1)
                        {
                            continue;
                        }
                        let top_anchor_pos = Vec3::new(top_anchor_x, top_anchor_y, anchor.pos.z);
                        if !line_passes_mesh_clearance(
                            top_anchor_pos,
                            anchor.pos,
                            meshes.get(&trunk.base.model_id),
                            anchor.diameter_mm,
                        ) {
                            continue;
                        }
                        let brace_radius = js_max(0.001, anchor.diameter_mm / 2.0);
                        if check_slanted_collision(
                            top_anchor_pos,
                            candidate_root_pos,
                            &trunk.base.model_id,
                            brace_radius + 1.0,
                        ) {
                            continue;
                        }

                        let host = KickstandHostTarget {
                            segment_id: anchor.segment_id.clone(),
                            support_kind: host_kind,
                            t: anchor.t,
                            pos: anchor.pos,
                            diameter_mm: anchor.diameter_mm,
                            min_t: None,
                        };
                        let Ok(trial) = build_kickstand_data(
                            &KickstandBuildInput {
                                model_id: &trunk.base.model_id,
                                root_pos: candidate_root_pos,
                                host: &host,
                                layout_overrides: None,
                                authored_column_top: None,
                                collision_mesh: None,
                                max_shaft_angle_deg: None,
                            },
                            live,
                            new_id,
                        ) else {
                            continue;
                        };
                        if !built_passes_clearance(&trial, &trunk.base.model_id) {
                            continue;
                        }
                        // The snuggest legal root: either side of an axis covers it.
                        let host_dist_sq = (candidate_root_pos.x - root_pos.x).powi(2)
                            + (candidate_root_pos.y - root_pos.y).powi(2);
                        if selected.as_ref().is_none_or(|s| host_dist_sq < s.2) {
                            selected = Some((trial, candidate_root_pos, host_dist_sq));
                        }
                    }
                    if selected.is_some() {
                        break;
                    }
                }

                let Some((build, selected_root_pos, _)) = selected else {
                    continue;
                };
                let actual_angle = normalize_axis_angle_rad(js_atan2(
                    selected_root_pos.y - root_pos.y,
                    selected_root_pos.x - root_pos.x,
                ));
                let group_axes = group_axes_by_group_id.entry(group_id.clone()).or_default();
                let need_before = additional_axes_needed_for_two_axis_bracing(
                    group_axes,
                    auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG,
                );
                let mut with_new = group_axes.clone();
                with_new.push(actual_angle);
                let need_after = additional_axes_needed_for_two_axis_bracing(
                    &with_new,
                    auto_bracing_hard_rules::MIN_AXIS_SEPARATION_DEG,
                );
                // Only a kickstand that improves the group's axis coverage.
                if need_after >= need_before {
                    continue;
                }
                generated.push(build);
                if is_grid_enabled {
                    occupied_grid_nodes.insert(grid_node_key(
                        &trunk.base.model_id,
                        snap_to_grid_index(selected_root_pos.x, grid.spacing_mm),
                        snap_to_grid_index(selected_root_pos.y, grid.spacing_mm),
                    ));
                }
                occupied_root_positions.push(selected_root_pos);
                generated_axes.push(actual_angle);
                group_axes.push(actual_angle);
                remaining_group_need = need_after;
                remaining_need_by_group_id.insert(group_id.clone(), remaining_group_need);
                if i == 0 {
                    existing_axis = actual_angle;
                }
                used_root_positions.push(selected_root_pos);
            }
        }
    }
    generated
}
