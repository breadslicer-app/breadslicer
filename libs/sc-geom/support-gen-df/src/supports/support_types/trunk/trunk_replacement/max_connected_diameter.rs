//! DragonFruit `src/supports/SupportTypes/Trunk/TrunkReplacement/maxConnectedDiameter.ts`,
//! the parts the auto-support plan uses: the member diameter the forest report
//! prints, the stepwise host diameter profile, and the forest-wide resize.
//! (`computeMaxConnectedDiameterFromTrunk`, the connected-graph walk, is the
//! store's and is ported with it.)

use indexmap::IndexMap;

use crate::supports::constants::get_joint_diameter;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::joint::joint_utils::{SplitHosts, split_support_shaft};
use crate::supports::support_primitives::knot::segment_endpoints::{
    EndpointHosts, ShaftEntity, resolve_segment_endpoints,
};
use crate::supports::support_type_registry::{SupportEntityValue, get_support_type_descriptor};
use crate::supports::types::{Branch, Knot, Leaf, SupportState, Trunk, Vec3};
use crate::three::math_utils::{js_max, js_min};

fn leaf_diameter(leaf: &Leaf) -> f64 {
    let p = &leaf.contact_cone.profile;
    js_max(p.body_diameter_mm, p.contact_diameter_mm)
}

fn widest_segment(segments: &[crate::supports::types::Segment]) -> f64 {
    segments
        .iter()
        .fold(0.0, |m, s| if s.diameter > m { s.diameter } else { m })
}

/// The diameter a support contributes to its connected graph: a shafted type
/// its widest segment, a brace its profile, a leaf its contact cone.
pub fn member_diameter_of(entity: &SupportEntityValue) -> f64 {
    let segments = match entity {
        SupportEntityValue::Trunk(e) => Some(&e.segments),
        SupportEntityValue::Branch(e) => Some(&e.segments),
        SupportEntityValue::Twig(e) => Some(&e.segments),
        SupportEntityValue::Stick(e) => Some(&e.segments),
        SupportEntityValue::Stump(e) => Some(&e.segments),
        SupportEntityValue::Kickstand(e) => Some(&e.segments),
        SupportEntityValue::Brace(_) | SupportEntityValue::Leaf(_) => None,
    };
    let type_id = match entity {
        SupportEntityValue::Trunk(_) => {
            crate::supports::support_type_registry::SupportTypeId::Trunk
        }
        SupportEntityValue::Branch(_) => {
            crate::supports::support_type_registry::SupportTypeId::Branch
        }
        SupportEntityValue::Leaf(_) => crate::supports::support_type_registry::SupportTypeId::Leaf,
        SupportEntityValue::Twig(_) => crate::supports::support_type_registry::SupportTypeId::Twig,
        SupportEntityValue::Stick(_) => {
            crate::supports::support_type_registry::SupportTypeId::Stick
        }
        SupportEntityValue::Brace(_) => {
            crate::supports::support_type_registry::SupportTypeId::Brace
        }
        SupportEntityValue::Stump(_) => {
            crate::supports::support_type_registry::SupportTypeId::Stump
        }
        SupportEntityValue::Kickstand(_) => {
            crate::supports::support_type_registry::SupportTypeId::Kickstand
        }
    };
    if get_support_type_descriptor(type_id).has_segments {
        return segments.map_or(0.0, |s| widest_segment(s));
    }
    match entity {
        SupportEntityValue::Brace(b) => b.profile.diameter,
        SupportEntityValue::Leaf(l) => leaf_diameter(l),
        _ => 0.0,
    }
}

/// Every segment at one diameter, its joints at the matching joint diameter.
pub fn apply_diameter_to_trunk(trunk: &Trunk, diameter_mm: f64) -> Trunk {
    if !diameter_mm.is_finite() || diameter_mm <= 0.0 {
        return trunk.clone();
    }
    let joint_diameter = get_joint_diameter(diameter_mm);
    let mut joint_by_id: IndexMap<String, f64> = IndexMap::new();
    let mut out = trunk.clone();
    for seg in &mut out.segments {
        if let Some(j) = &mut seg.top_joint {
            j.diameter = joint_by_id.get(&j.id).copied().unwrap_or(joint_diameter);
            joint_by_id.insert(j.id.clone(), j.diameter);
        }
        if let Some(j) = &mut seg.bottom_joint {
            j.diameter = joint_by_id.get(&j.id).copied().unwrap_or(joint_diameter);
            joint_by_id.insert(j.id.clone(), j.diameter);
        }
        seg.diameter = diameter_mm;
    }
    out
}

/// The largest finite value, at least 0.
fn max_finite(values: &[Option<f64>]) -> f64 {
    values
        .iter()
        .flatten()
        .filter(|v| v.is_finite())
        .fold(0.0, |m, &v| js_max(m, v))
}

fn branch_demand_diameter_mm(branch: &Branch) -> f64 {
    branch
        .segments
        .iter()
        .fold(0.0, |m, s| js_max(m, s.diameter))
}

fn infer_trunk_base_diameter_mm(trunk: &Trunk, over: Option<f64>, live: &SupportSettings) -> f64 {
    let mut candidates: Vec<Option<f64>> = vec![over, trunk.base_diameter_mm];
    let min_seg = trunk
        .segments
        .iter()
        .filter(|s| s.diameter.is_finite() && s.diameter > 0.0)
        .fold(f64::INFINITY, |m, s| js_min(m, s.diameter));
    if min_seg != f64::INFINITY {
        candidates.push(Some(min_seg));
    }
    candidates.push(Some(live.shaft.diameter_mm));
    candidates
        .into_iter()
        .flatten()
        .find(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.0)
}

fn trunk_contact_demand_diameter_mm(
    trunk: &Trunk,
    base: Option<f64>,
    live: &SupportSettings,
) -> f64 {
    let top_shaft_dia = infer_trunk_base_diameter_mm(trunk, base, live);
    let cone_demand = trunk.contact_cone.as_ref().map_or(0.0, |c| {
        max_finite(&[
            Some(c.profile.body_diameter_mm),
            Some(c.profile.contact_diameter_mm),
        ])
    });
    js_max(js_max(0.0, top_shaft_dia), cone_demand)
}

fn compute_linear_t(pos: Vec3, start: Vec3, end: Vec3) -> f64 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let dz = end.z - start.z;
    let len_sq = dx * dx + dy * dy + dz * dz;
    if len_sq < 0.000001 {
        return 0.0;
    }
    let t = ((pos.x - start.x) * dx + (pos.y - start.y) * dy + (pos.z - start.z) * dz) / len_sq;
    js_min(1.0, js_max(0.0, t))
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrunkKnotUpdate {
    pub before: Knot,
    pub after: Knot,
}

/// Re-solve a host's stepwise shaft diameter from the members it carries.
/// Segment boundaries split at each branch-carrying knot, and the knots move
/// with them. `live` is the global `getSettings()`.
pub fn compute_and_apply_support_diameter_profile(
    snapshot: &SupportState,
    trunk_id: &str,
    base_shaft_diameter_mm: Option<f64>,
    live: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> Option<(Trunk, Vec<TrunkKnotUpdate>)> {
    let c = &snapshot.collections;
    let trunk = c.trunks.get(trunk_id)?;
    let root = c.roots.get(&trunk.root_id)?;

    let mut next_trunk = trunk.clone();
    let mut next_knot_by_id: IndexMap<String, Knot> = c
        .knots
        .values()
        .map(|k| (k.id.clone(), k.clone()))
        .collect();
    let mut before_by_knot_id: IndexMap<String, Knot> = IndexMap::new();
    let record_update = |after: Knot,
                         next: &mut IndexMap<String, Knot>,
                         before_map: &mut IndexMap<String, Knot>| {
        let Some(before) = c.knots.get(&after.id) else {
            return;
        };
        before_map
            .entry(after.id.clone())
            .or_insert_with(|| before.clone());
        next.insert(after.id.clone(), after);
    };

    let mut branches_by_parent: IndexMap<&str, Vec<&Branch>> = IndexMap::new();
    for b in c.branches.values() {
        if b.parent_knot_id.is_empty() {
            continue;
        }
        branches_by_parent
            .entry(&b.parent_knot_id)
            .or_default()
            .push(b);
    }
    let mut leaves_by_parent: IndexMap<&str, Vec<&Leaf>> = IndexMap::new();
    for l in c.leaves.values() {
        if l.parent_knot_id.is_empty() {
            continue;
        }
        leaves_by_parent
            .entry(&l.parent_knot_id)
            .or_default()
            .push(l);
    }

    let eps_t = 1e-6;
    let mut with_branches: Vec<&Knot> = c
        .knots
        .values()
        .filter(|k| trunk.segments.iter().any(|s| s.id == k.parent_shaft_id))
        .filter(|k| {
            branches_by_parent
                .get(k.id.as_str())
                .is_some_and(|v| !v.is_empty())
        })
        .collect();
    with_branches.sort_by(|a, b| {
        (b.pos.z - a.pos.z)
            .partial_cmp(&0.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Split segments top-down so each branch-attached knot is a boundary.
    for original in with_branches {
        let Some(knot) = next_knot_by_id.get(&original.id).cloned() else {
            continue;
        };
        let Some(seg_index) = next_trunk
            .segments
            .iter()
            .position(|s| s.id == knot.parent_shaft_id)
        else {
            continue;
        };
        let seg = next_trunk.segments[seg_index].clone();
        let endpoints = resolve_segment_endpoints(
            ShaftEntity::Trunk(&next_trunk),
            &seg,
            seg_index,
            EndpointHosts {
                root: Some(root),
                host_knot: None,
            },
        );
        let existing_t = match knot.t {
            Some(t) => js_min(1.0, js_max(0.0, t)),
            None => endpoints.map_or(0.0, |e| compute_linear_t(knot.pos, e.start, e.end)),
        };

        // A knot on the boundary belongs to the thicker side (below).
        if existing_t <= eps_t {
            if seg_index > 0 {
                let prev_id = next_trunk.segments[seg_index - 1].id.clone();
                let after = Knot {
                    parent_shaft_id: prev_id,
                    t: Some(1.0),
                    ..knot.clone()
                };
                if after.parent_shaft_id != knot.parent_shaft_id || after.t != knot.t {
                    record_update(after, &mut next_knot_by_id, &mut before_by_knot_id);
                }
            }
            continue;
        }
        if existing_t >= 1.0 - eps_t {
            let after = Knot {
                t: Some(1.0),
                ..knot.clone()
            };
            if after.t != knot.t {
                record_update(after, &mut next_knot_by_id, &mut before_by_knot_id);
            }
            continue;
        }

        let split_t = existing_t;
        let seg_id_to_split = seg.id.clone();
        let (after_split, _) = split_support_shaft(
            &next_trunk,
            &seg_id_to_split,
            knot.pos,
            Some(split_t),
            SplitHosts {
                root: Some(root),
                host_knot: None,
            },
            None,
            new_id,
        );
        let Some(bottom_index) = after_split
            .segments
            .iter()
            .position(|s| s.id == seg_id_to_split)
        else {
            next_trunk = after_split;
            continue;
        };
        let Some(top_seg) = after_split.segments.get(bottom_index + 1) else {
            next_trunk = after_split;
            continue;
        };
        let top_seg_id = top_seg.id.clone();

        // Rehost/rescale every knot that was on the split segment.
        let on_split: Vec<Knot> = next_knot_by_id
            .values()
            .filter(|k| k.parent_shaft_id == seg_id_to_split)
            .cloned()
            .collect();
        for k in on_split {
            let kt = match k.t {
                Some(t) => js_min(1.0, js_max(0.0, t)),
                None => endpoints.map_or(0.0, |e| compute_linear_t(k.pos, e.start, e.end)),
            };
            let after = if (kt - split_t).abs() <= eps_t {
                Knot {
                    parent_shaft_id: seg_id_to_split.clone(),
                    t: Some(1.0),
                    ..k.clone()
                }
            } else if kt < split_t {
                Knot {
                    parent_shaft_id: seg_id_to_split.clone(),
                    t: Some(if split_t <= eps_t { 0.0 } else { kt / split_t }),
                    ..k.clone()
                }
            } else {
                let denom = 1.0 - split_t;
                Knot {
                    parent_shaft_id: top_seg_id.clone(),
                    t: Some(if denom <= eps_t {
                        0.0
                    } else {
                        (kt - split_t) / denom
                    }),
                    ..k.clone()
                }
            };
            if after.parent_shaft_id != k.parent_shaft_id || after.t != k.t {
                record_update(after, &mut next_knot_by_id, &mut before_by_knot_id);
            }
        }

        next_trunk = after_split;
    }

    // Demand at the top of each segment from any member-attached knot on it.
    let trunk_seg_ids: std::collections::HashSet<&str> =
        next_trunk.segments.iter().map(|s| s.id.as_str()).collect();
    let mut demand_at_top: IndexMap<String, f64> = IndexMap::new();
    for knot in next_knot_by_id.values() {
        if !trunk_seg_ids.contains(knot.parent_shaft_id.as_str()) {
            continue;
        }
        let branches = branches_by_parent.get(knot.id.as_str());
        let leaves = leaves_by_parent.get(knot.id.as_str());
        let n = branches.map_or(0, Vec::len) + leaves.map_or(0, Vec::len);
        if n == 0 {
            continue;
        }
        let mut demand = 0.0;
        for b in branches.into_iter().flatten() {
            demand = js_max(demand, branch_demand_diameter_mm(b));
        }
        for l in leaves.into_iter().flatten() {
            demand = js_max(demand, leaf_diameter(l));
        }
        let prev = demand_at_top
            .get(&knot.parent_shaft_id)
            .copied()
            .unwrap_or(0.0);
        if demand > prev {
            demand_at_top.insert(knot.parent_shaft_id.clone(), demand);
        }
    }

    // Stepwise diameters top -> bottom.
    let n = next_trunk.segments.len();
    let mut seg_diameters = vec![0.0; n];
    let mut running_max =
        trunk_contact_demand_diameter_mm(&next_trunk, base_shaft_diameter_mm, live);
    for i in (0..n).rev() {
        let local = demand_at_top
            .get(&next_trunk.segments[i].id)
            .copied()
            .unwrap_or(0.0);
        running_max = js_max(running_max, local);
        seg_diameters[i] = running_max;
    }

    // Joint diameters match the thicker adjacent segment.
    let mut joint_diameter_by_id: IndexMap<String, f64> = IndexMap::new();
    for i in 0..n {
        let seg = &next_trunk.segments[i];
        let seg_dia = seg_diameters[i];
        if let Some(j) = &seg.bottom_joint {
            let below = if i > 0 { seg_diameters[i - 1] } else { seg_dia };
            let candidate = get_joint_diameter(js_max(seg_dia, below));
            let prev = joint_diameter_by_id.get(&j.id).copied().unwrap_or(0.0);
            joint_diameter_by_id.insert(j.id.clone(), js_max(prev, candidate));
        }
        if let Some(j) = &seg.top_joint {
            let above = if i + 1 < n {
                seg_diameters[i + 1]
            } else {
                seg_dia
            };
            let candidate = get_joint_diameter(js_max(seg_dia, above));
            let prev = joint_diameter_by_id.get(&j.id).copied().unwrap_or(0.0);
            joint_diameter_by_id.insert(j.id.clone(), js_max(prev, candidate));
        }
    }

    for (idx, seg) in next_trunk.segments.iter_mut().enumerate() {
        seg.diameter = seg_diameters[idx];
        if let Some(j) = &mut seg.top_joint {
            j.diameter = joint_diameter_by_id
                .get(&j.id)
                .copied()
                .unwrap_or(j.diameter);
        }
        if let Some(j) = &mut seg.bottom_joint {
            j.diameter = joint_diameter_by_id
                .get(&j.id)
                .copied()
                .unwrap_or(j.diameter);
        }
    }

    let knot_updates = before_by_knot_id
        .into_iter()
        .filter_map(|(id, before)| {
            next_knot_by_id.get(&id).map(|after| TrunkKnotUpdate {
                before,
                after: after.clone(),
            })
        })
        .collect();
    Some((next_trunk, knot_updates))
}

/// Forest-wide diameter resize: every trunk's stepwise profile re-derived
/// from its final attachment tree, trunks in order so a later one sees the
/// earlier rehosts.
pub fn compute_forest_diameter_profile(
    snapshot: &SupportState,
    live: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> SupportState {
    let mut working = snapshot.clone();
    let ids: Vec<String> = snapshot.collections.trunks.keys().cloned().collect();
    for trunk_id in ids {
        let Some((trunk, updates)) =
            compute_and_apply_support_diameter_profile(&working, &trunk_id, None, live, new_id)
        else {
            continue;
        };
        for u in updates {
            working
                .collections
                .knots
                .insert(u.after.id.clone(), u.after);
        }
        working
            .collections
            .trunks
            .insert(trunk.base.id.clone(), trunk);
    }
    working
}
