//! DragonFruit `src/supports/SupportTypes/Trunk/useTrunkPlacement.ts`: the
//! `buildCavityBridge` fallback the auto pass calls. The React hook around it
//! is the GUI's.
//!
//! When a trunk cannot reach the plate (the tip is inside a closed cavity),
//! find the cavity floor by raycasting down and bridge the two contacts. Which
//! type bridges them is the registry's call, by contact span.

use super::super::contact_bridge::{BridgeEntity, ContactBridgeRequest, build_contact_bridge};
use crate::supports::model_mesh::ModelMesh;
use crate::supports::placement_logic::collision_avoidance::is_shaft_blocked;
use crate::supports::placement_logic::collision_utils::{
    check_short_bridge_collision, tip_cone_crosses_model,
};
use crate::supports::rendering::support_builder::{SupportData, support_data_for_entity};
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::knot::segment_endpoints::ShaftEntity;
use crate::supports::support_type_registry::{
    SupportPlacementMetric, SupportPlacementSettingPath, SupportTypeId, bridge_may_land_sideways,
    get_support_type_descriptor, select_type_for_placement,
};
use crate::supports::types::Vec3;
use crate::three::math_utils::{js_cos, js_sin};
use crate::three::{Matrix3, Ray};
use crate::three_mesh_bvh::Side;

/// The auto pass's tier sizing for the bridge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CavityBridgeSizing {
    pub tip_contact_diameter_mm: f64,
    pub shaft_diameter_mm: f64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CavityBridgeResult {
    pub kind: SupportTypeId,
    pub support_data: SupportData,
    pub entity: BridgeEntity,
}

/// One hit below the tip, with its world normal.
#[derive(Clone, Copy)]
struct Candidate {
    point: Vec3,
    normal: Vec3,
}

/// Why a contact has no bridge, or why a place to land one was turned down
/// (SoulCrafted, #445).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BridgeRefusal {
    /// Nothing of the model lies under the contact.
    NoSurface,
    /// No support type bridges that span.
    NoType,
    /// A landing out to the side, which that type may not reach.
    Sideways,
    /// The shaft leans past what its type stands.
    Canted,
    /// The shaft runs into the model.
    Blocked,
    /// The contact lies on a top surface, its normal turned over to look
    /// into the model: nothing hangs there, and a bridge from it would run
    /// through the model and end in the air.
    TopSurface,
}

/// What looking for a bridge found (SoulCrafted, #445).
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeAttempt {
    pub bridge: Option<CavityBridgeResult>,
    /// Why each landing before the one taken was turned down, in order.
    pub refusals: Vec<BridgeRefusal>,
}

/// The cavity bridge, or None when no lower surface is found or the bridge is
/// not worth placing. `side` is the mesh material's raycast side; `settings`
/// is the live `getSettings()`.
#[allow(clippy::too_many_arguments)]
pub fn build_cavity_bridge(
    tip_pos: Vec3,
    tip_normal: Vec3,
    model_id: &str,
    mesh: ModelMesh<'_>,
    side: Side,
    sizing: Option<CavityBridgeSizing>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> Option<CavityBridgeResult> {
    attempt_cavity_bridge(
        tip_pos, tip_normal, model_id, mesh, side, sizing, settings, new_id,
    )
    .bridge
}

/// [`build_cavity_bridge`], saying why each landing was turned down.
///
/// DragonFruit lands on the first surface its search likes and stops there.
/// With [`MeshToMeshSettings::every_landing`] the other surfaces around the
/// contact are tried after it, in the order the search meets them: under a
/// contact, one slanted wall says nothing of the rest (#445). Each is held to
/// the same gates, and what the first landing gave never changes.
///
/// [`MeshToMeshSettings::every_landing`]: crate::supports::settings::types::MeshToMeshSettings::every_landing
#[allow(clippy::too_many_arguments)]
pub fn attempt_cavity_bridge(
    tip_pos: Vec3,
    tip_normal: Vec3,
    model_id: &str,
    mesh: ModelMesh<'_>,
    side: Side,
    sizing: Option<CavityBridgeSizing>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> BridgeAttempt {
    // Offset origin slightly inward along tip normal so we don't self-hit the
    // surface we just clicked, then nudge down past it.
    const OFFSET_MM: f64 = 0.5;
    let mut base_origin = tip_pos.add_scaled_vector(tip_normal, OFFSET_MM);
    base_origin.z -= OFFSET_MM * 0.1;

    // Prefer a true "floor" hit (normal has meaningful +Z); fall back to any
    // below-tip hit only if no floor-like surface is found.
    const BELOW_EPS_MM: f64 = 0.1;
    const FLOOR_Z_MIN: f64 = 0.35;
    const MAX_HIT_SCAN: usize = 64;
    let normal_matrix = Matrix3::normal_matrix(mesh.rays.matrix_world);

    if settings.mesh_to_mesh.every_landing
        && looks_into_the_model(tip_pos, tip_normal, mesh, &normal_matrix)
    {
        return BridgeAttempt {
            bridge: None,
            refusals: vec![BridgeRefusal::TopSurface],
        };
    }

    let scan_down = |ox: f64, oy: f64| -> (Option<Candidate>, Option<Candidate>) {
        let ray = Ray {
            origin: Vec3::new(ox, oy, base_origin.z),
            direction: Vec3::new(0.0, 0.0, -1.0),
        };
        let mut floor = None;
        let mut first = None;
        for hit in mesh
            .rays
            .intersect_object(&ray, side, 0.0, f64::INFINITY, false)
            .into_iter()
            .take(MAX_HIT_SCAN)
        {
            if hit.point.z >= tip_pos.z - BELOW_EPS_MM {
                continue;
            }
            let n = hit
                .face
                .normal
                .apply_normal_matrix(&normal_matrix)
                .normalize();
            let candidate = Candidate {
                point: hit.point,
                normal: n,
            };
            if first.is_none() {
                first = Some(candidate);
            }
            if n.z >= FLOOR_Z_MIN {
                floor = Some(candidate);
                break;
            }
        }
        (floor, first)
    };

    // Straight down first, then a small disc around it. Nearest radius wins.
    let cutoff = settings.mesh_to_mesh.stick_vs_twig_cutoff_mm;
    const NEAR_RADII_MM: [f64; 4] = [0.0, 0.75, 1.5, 2.25];
    let near_max = NEAR_RADII_MM[NEAR_RADII_MM.len() - 1];
    // A twig's own reach: a lateral offset up to its maximum length.
    let twig_reach_radii: Vec<f64> = (0..(cutoff / 0.75).floor().max(0.0) as usize)
        .map(|i| 0.75 * (i as f64 + 1.0))
        .filter(|&r| r > near_max)
        .collect();
    let run_search = |radii: &[f64]| -> (Option<Candidate>, bool) {
        let mut first_below = None;
        for &radius_mm in radii {
            let steps = if radius_mm == 0.0 { 1 } else { 8 };
            for i in 0..steps {
                let angle = (f64::from(i) / f64::from(steps)) * std::f64::consts::PI * 2.0;
                let (floor, first) = scan_down(
                    base_origin.x + js_cos(angle) * radius_mm,
                    base_origin.y + js_sin(angle) * radius_mm,
                );
                if let Some(floor) = floor {
                    return (Some(floor), radius_mm > near_max);
                }
                if first_below.is_none() {
                    first_below = first;
                }
            }
        }
        (first_below, false)
    };

    // Over a gap narrower than `OFFSET_MM` the search starts inside the floor,
    // and that floor is the landing: it is looked for from the tip (#445). A
    // gap under `NARROWEST_GAP_MM` is a slot of the model, and it stays one.
    let under_the_tip = || -> Option<Candidate> {
        let ray = Ray {
            origin: tip_pos,
            direction: Vec3::new(0.0, 0.0, -1.0),
        };
        mesh.rays
            .intersect_object(&ray, side, 0.0, OFFSET_MM + BELOW_EPS_MM, false)
            .into_iter()
            .find(|hit| hit.point.z < tip_pos.z - BELOW_EPS_MM)
            .filter(|hit| hit.point.z <= tip_pos.z - NARROWEST_GAP_MM)
            .map(|hit| Candidate {
                point: hit.point,
                normal: hit
                    .face
                    .normal
                    .apply_normal_matrix(&normal_matrix)
                    .normalize(),
            })
            .filter(|landing| landing.normal.z >= FLOOR_Z_MIN)
    };
    let narrow = settings
        .mesh_to_mesh
        .every_landing
        .then(under_the_tip)
        .flatten();
    let (mut chosen, mut reached_sideways) = match narrow {
        Some(floor) => (Some(floor), false),
        None => run_search(&NEAR_RADII_MM),
    };
    if chosen.is_none() {
        // Nothing straight down: a TWIG may still prop the contact off a
        // neighbouring surface, so the search may reach that far sideways.
        (chosen, reached_sideways) = run_search(&twig_reach_radii);
    }
    let Some(first) = chosen else {
        return BridgeAttempt {
            bridge: None,
            refusals: vec![BridgeRefusal::NoSurface],
        };
    };

    let read_setting = |path: SupportPlacementSettingPath| match path {
        SupportPlacementSettingPath::MeshToMeshStickVsTwigCutoffMm => {
            Some(settings.mesh_to_mesh.stick_vs_twig_cutoff_mm)
        }
    };
    let mut land =
        |chosen: Candidate, reached_sideways: bool| -> Result<CavityBridgeResult, BridgeRefusal> {
            let b_pos = chosen.point;
            let b_normal = chosen.normal;
            let dx = tip_pos.x - b_pos.x;
            let dy = tip_pos.y - b_pos.y;
            let dz = tip_pos.z - b_pos.z;
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            let kind =
                select_type_for_placement(SupportPlacementMetric::ContactSpan, dist, &read_setting)
                    .ok_or(BridgeRefusal::NoType)?;

            // Landing beyond the near cutoff on the wide search is a lateral prop, which
            // only a type declaring `mayReachSideways` may build.
            if !bridge_may_land_sideways(kind, dist, cutoff, reached_sideways) {
                return Err(BridgeRefusal::Sideways);
            }

            let built = build_contact_bridge(
                kind,
                &ContactBridgeRequest {
                    model_id,
                    a_pos: tip_pos,
                    a_normal: tip_normal,
                    b_pos,
                    b_normal,
                    shaft_diameter_mm: sizing.map(|s| s.shaft_diameter_mm),
                    tip_contact_diameter_mm: sizing.map(|s| s.tip_contact_diameter_mm),
                    mesh: None,
                    manual: None,
                },
                settings,
                &mut *new_id,
            )
            .ok_or(BridgeRefusal::Canted)?;
            let entity = built.entity;
            // The SHORT bridge is the type whose contact-span rule is bounded above.
            let short_bridge = get_support_type_descriptor(kind)
                .placement_rule
                .is_some_and(|rule| rule.max_mm.is_some());

            // The shaft must not pierce the model (trunk post-cull clearance).
            let segments = match &entity {
                BridgeEntity::Twig(t) => &t.segments,
                BridgeEntity::Stick(s) => &s.segments,
            };
            let seg = segments.first();
            let start = seg
                .and_then(|s| s.bottom_joint.as_ref())
                .map_or(b_pos, |j| j.pos);
            let end = seg
                .and_then(|s| s.top_joint.as_ref())
                .map_or(tip_pos, |j| j.pos);
            let radius = seg.map_or_else(
                || sizing.map_or(1.0, |s| s.shaft_diameter_mm),
                |s| s.diameter,
            ) / 2.0
                + 0.15;
            // Ray-based for a twig, like buildTwig: the SDF reads the thin gap as material.
            let blocked = if short_bridge {
                check_short_bridge_collision(start, end, radius, mesh.rays).hit
            } else {
                is_shaft_blocked(start, end, radius, mesh.field)
            };
            if blocked {
                return Err(BridgeRefusal::Blocked);
            }
            // Nor may a tip cone of it go through the model (#500).
            if settings.auto_support.tip_cones_clear
                && let BridgeEntity::Stick(stick) = &entity
                && [&stick.contact_cone_a, &stick.contact_cone_b]
                    .into_iter()
                    .any(|cone| tip_cone_crosses_model(cone, mesh.rays))
            {
                return Err(BridgeRefusal::Blocked);
            }

            let support_data = match &entity {
                BridgeEntity::Twig(t) => support_data_for_entity(ShaftEntity::Twig(t)),
                BridgeEntity::Stick(s) => support_data_for_entity(ShaftEntity::Stick(s)),
            };
            Ok(CavityBridgeResult {
                kind,
                support_data,
                entity,
            })
        };

    let mut refusals = Vec::new();
    match land(first, reached_sideways) {
        Ok(bridge) => {
            return BridgeAttempt {
                bridge: Some(bridge),
                refusals,
            };
        }
        Err(why) => refusals.push(why),
    }
    if !settings.mesh_to_mesh.every_landing {
        return BridgeAttempt {
            bridge: None,
            refusals,
        };
    }

    // Every other surface around the contact, as the search would meet it:
    // the floors first, the rest after them.
    let mut floors = Vec::new();
    let mut others = Vec::new();
    for &radius_mm in NEAR_RADII_MM.iter().chain(&twig_reach_radii) {
        let steps = if radius_mm == 0.0 { 1 } else { 8 };
        for i in 0..steps {
            let angle = (f64::from(i) / f64::from(steps)) * std::f64::consts::PI * 2.0;
            let (floor, below) = scan_down(
                base_origin.x + js_cos(angle) * radius_mm,
                base_origin.y + js_sin(angle) * radius_mm,
            );
            match (floor, below) {
                (Some(floor), _) => floors.push((floor, radius_mm > near_max)),
                (None, Some(below)) => others.push((below, false)),
                (None, None) => {}
            }
        }
    }
    for (landing, sideways) in floors.into_iter().chain(others) {
        if landing.point == first.point {
            continue;
        }
        match land(landing, sideways) {
            Ok(bridge) => {
                return BridgeAttempt {
                    bridge: Some(bridge),
                    refusals,
                };
            }
            Err(why) => refusals.push(why),
        }
    }
    BridgeAttempt {
        bridge: None,
        refusals,
    }
}

/// The least air a bridge goes into from the tip down, in millimetres: what its
/// two tips sink into the model. In less than that it would be no bridge but a
/// filling of the gap, and the model is not touched.
const NARROWEST_GAP_MM: f64 = 0.4;

/// Whether the normal of a contact at `tip_pos` looks into the model: from a
/// step along it, the way back to the contact goes out through the back of
/// the surface.
fn looks_into_the_model(
    tip_pos: Vec3,
    tip_normal: Vec3,
    mesh: ModelMesh<'_>,
    normal_matrix: &Matrix3,
) -> bool {
    const STEP_MM: f64 = 0.1;
    let normal = tip_normal.normalize();
    let ray = Ray {
        origin: tip_pos.add_scaled_vector(normal, STEP_MM),
        direction: normal.multiply_scalar(-1.0),
    };
    mesh.rays
        .intersect_object(&ray, Side::Double, 0.0, 2.0 * STEP_MM, false)
        .first()
        .is_some_and(|hit| {
            hit.face
                .normal
                .apply_normal_matrix(normal_matrix)
                .dot(normal)
                < 0.0
        })
}
