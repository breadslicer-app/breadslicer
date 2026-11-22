//! The bridge that stands in for a trunk whose contact cannot reach the
//! plate, such as one inside a closed cavity: the surface below the contact
//! is found with rays, and the two are joined by whichever type the registry
//! picks for that span.

use super::super::contact_bridge::{BridgeEntity, ContactBridgeRequest, build_contact_bridge};
use crate::bvh::Side;
use crate::geom::math_utils::{js_cos, js_sin};
use crate::geom::{Matrix3, Ray};
use crate::supports::model_mesh::ModelMesh;
use crate::supports::placement_logic::collision_avoidance::is_shaft_blocked;
use crate::supports::placement_logic::collision_utils::{
    check_short_bridge_collision, tip_cone_crosses_model,
};
use crate::supports::rendering::support_builder::{SupportData, support_data_for_entity};
use crate::supports::settings::types::SupportSettings;
use crate::supports::shafts::ShaftEntity;
use crate::supports::support_kinds::{
    SupportPlacementMetric, SupportPlacementSettingPath, SupportTypeId, bridge_may_land_sideways,
    get_support_type_descriptor, select_type_for_placement,
};
use crate::supports::types::Vec3;

/// How far off the surface the downward search starts, so it does not hit
/// the contact's own face.
const START_OFFSET_MM: f64 = 0.5;
/// A hit must lie at least this far below the contact to count.
const BELOW_MM: f64 = 0.1;
/// A surface whose normal rises at least this much counts as a floor.
const FLOOR_MIN_NORMAL_Z: f64 = 0.35;
/// The most hits read along one ray.
const MAX_HITS: usize = 64;
/// Search radii around the contact, straight down first.
const NEAR_RADII_MM: [f64; 4] = [0.0, 0.75, 1.5, 2.25];
const NEAR_REACH_MM: f64 = NEAR_RADII_MM[NEAR_RADII_MM.len() - 1];
/// The step between search radii, in mm.
const RADIUS_STEP_MM: f64 = 0.75;
/// The least air under a contact for a bridge to go into, in mm. Across less
/// the bridge's two tips would fill the gap rather than span it, and the
/// model is left alone.
const NARROWEST_GAP_MM: f64 = 0.4;

/// The auto pass's tier sizes for the bridge.
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

/// Why a contact gets no bridge, or why one landing for it was turned down
/// (#445).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BridgeRefusal {
    /// No part of the model lies under the contact.
    NoSurface,
    /// No support type bridges that span.
    NoType,
    /// The landing is off to the side, which that type may not reach.
    Sideways,
    /// The shaft leans past what its type allows.
    Canted,
    /// The shaft runs into the model.
    Blocked,
    /// The contact is on a top surface whose normal faces into the model:
    /// nothing hangs there, and a bridge from it would run through the model
    /// and end in the air.
    TopSurface,
}

/// What the search for a bridge found (#445).
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeAttempt {
    pub bridge: Option<CavityBridgeResult>,
    /// Why each landing tried before the one taken was turned down, in order.
    pub refusals: Vec<BridgeRefusal>,
}

/// A surface point under the contact, with its world normal.
#[derive(Clone, Copy)]
struct Landing {
    point: Vec3,
    normal: Vec3,
}

/// The cavity bridge for a contact, or `None` when nothing lies below it or
/// no bridge there is worth placing. `side` is the side the mesh's rays hit.
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

/// What looks for landings under one contact.
struct Search<'a> {
    tip_pos: Vec3,
    /// Where the downward rays start, above the contact's own face.
    origin: Vec3,
    mesh: ModelMesh<'a>,
    side: Side,
    normal_matrix: Matrix3,
    /// Radii past the near ones, out to a twig's reach.
    far_radii: Vec<f64>,
}

impl Search<'_> {
    fn landing(&self, point: Vec3, face_normal: Vec3) -> Landing {
        Landing {
            point,
            normal: face_normal
                .apply_normal_matrix(&self.normal_matrix)
                .normalize(),
        }
    }

    /// Straight down from `(x, y)`: the first floor below the contact, and
    /// the first surface of any kind below it.
    fn scan(&self, x: f64, y: f64) -> (Option<Landing>, Option<Landing>) {
        let ray = Ray {
            origin: Vec3::new(x, y, self.origin.z),
            direction: Vec3::new(0.0, 0.0, -1.0),
        };
        let mut first = None;
        for hit in self
            .mesh
            .rays
            .intersect_object(&ray, self.side, 0.0, f64::INFINITY, false)
            .into_iter()
            .take(MAX_HITS)
        {
            if hit.point.z >= self.tip_pos.z - BELOW_MM {
                continue;
            }
            let landing = self.landing(hit.point, hit.face.normal);
            first = first.or(Some(landing));
            if landing.normal.z >= FLOOR_MIN_NORMAL_Z {
                return (Some(landing), first);
            }
        }
        (None, first)
    }

    /// Every ray position for `radii`, in order: one in the middle for a
    /// zero radius, eight around for any other. Each comes with whether it
    /// lies beyond the near reach.
    fn ring_points(&self, radii: &[f64]) -> Vec<(f64, f64, bool)> {
        let mut points = Vec::new();
        for &radius in radii {
            let steps = if radius == 0.0 { 1 } else { 8 };
            for i in 0..steps {
                let angle = (f64::from(i) / f64::from(steps)) * std::f64::consts::PI * 2.0;
                points.push((
                    self.origin.x + js_cos(angle) * radius,
                    self.origin.y + js_sin(angle) * radius,
                    radius > NEAR_REACH_MM,
                ));
            }
        }
        points
    }

    /// The first floor met across `radii`, and whether it lies beyond the
    /// near reach; without a floor, the first surface met at all.
    fn first_floor(&self, radii: &[f64]) -> (Option<Landing>, bool) {
        let mut fallback = None;
        for (x, y, far) in self.ring_points(radii) {
            let (floor, first) = self.scan(x, y);
            if let Some(floor) = floor {
                return (Some(floor), far);
            }
            fallback = fallback.or(first);
        }
        (fallback, false)
    }

    /// A floor right under the contact when the gap is narrower than the
    /// start offset, so the search would begin inside that floor (#445). A
    /// gap under the narrowest is a slot of the model, and stays one.
    fn floor_just_below(&self) -> Option<Landing> {
        let ray = Ray {
            origin: self.tip_pos,
            direction: Vec3::new(0.0, 0.0, -1.0),
        };
        self.mesh
            .rays
            .intersect_object(&ray, self.side, 0.0, START_OFFSET_MM + BELOW_MM, false)
            .into_iter()
            .find(|hit| hit.point.z < self.tip_pos.z - BELOW_MM)
            .filter(|hit| hit.point.z <= self.tip_pos.z - NARROWEST_GAP_MM)
            .map(|hit| self.landing(hit.point, hit.face.normal))
            .filter(|landing| landing.normal.z >= FLOOR_MIN_NORMAL_Z)
    }

    /// The landing to try first, and whether it lies beyond the near reach:
    /// a floor just below, else the nearest floor around, else the nearest
    /// floor out to a twig's reach, which may prop the contact off a
    /// neighbouring surface.
    fn preferred(&self) -> Option<(Landing, bool)> {
        if let Some(floor) = self.floor_just_below() {
            return Some((floor, false));
        }
        let (near, far) = self.first_floor(&NEAR_RADII_MM);
        let (found, far) = match near {
            Some(landing) => (Some(landing), far),
            None => self.first_floor(&self.far_radii),
        };
        found.map(|landing| (landing, far))
    }

    /// Every other landing around the contact, as the search meets them:
    /// floors first, then the other surfaces.
    fn alternatives(&self) -> Vec<(Landing, bool)> {
        let radii: Vec<f64> = NEAR_RADII_MM
            .iter()
            .chain(&self.far_radii)
            .copied()
            .collect();
        let mut floors = Vec::new();
        let mut others = Vec::new();
        for (x, y, far) in self.ring_points(&radii) {
            match self.scan(x, y) {
                (Some(floor), _) => floors.push((floor, far)),
                (None, Some(surface)) => others.push((surface, false)),
                (None, None) => {}
            }
        }
        floors.extend(others);
        floors
    }
}

/// [`build_cavity_bridge`], also saying why each landing was turned down.
///
/// When the first landing is refused, the other surfaces around the contact
/// are tried after it, in the order the search meets them: one slanted wall
/// under a contact says nothing of the rest (#445). Each is held to the same
/// tests, and the first landing's outcome is never changed by them.
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
    let refused = |why: BridgeRefusal| BridgeAttempt {
        bridge: None,
        refusals: vec![why],
    };
    let normal_matrix = Matrix3::normal_matrix(mesh.rays.matrix_world);
    if faces_into_model(tip_pos, tip_normal, mesh, &normal_matrix) {
        return refused(BridgeRefusal::TopSurface);
    }

    let cutoff = settings.mesh_to_mesh.stick_vs_twig_cutoff_mm;
    let mut origin = tip_pos.add_scaled_vector(tip_normal, START_OFFSET_MM);
    origin.z -= START_OFFSET_MM * 0.1;
    let search = Search {
        tip_pos,
        origin,
        mesh,
        side,
        normal_matrix,
        far_radii: (0..(cutoff / RADIUS_STEP_MM).floor().max(0.0) as usize)
            .map(|i| RADIUS_STEP_MM * (i as f64 + 1.0))
            .filter(|&r| r > NEAR_REACH_MM)
            .collect(),
    };
    let Some((first, first_far)) = search.preferred() else {
        return refused(BridgeRefusal::NoSurface);
    };

    let mut land = |landing: Landing, far: bool| {
        land_bridge(
            tip_pos, tip_normal, model_id, mesh, sizing, settings, new_id, landing, far,
        )
    };
    let mut refusals = Vec::new();
    match land(first, first_far) {
        Ok(bridge) => {
            return BridgeAttempt {
                bridge: Some(bridge),
                refusals,
            };
        }
        Err(why) => refusals.push(why),
    }
    for (landing, far) in search.alternatives() {
        if landing.point == first.point {
            continue;
        }
        match land(landing, far) {
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

/// The bridge from the contact down to `landing`, or why there is none.
/// `far` says the landing was found beyond the near reach.
#[allow(clippy::too_many_arguments)]
fn land_bridge(
    tip_pos: Vec3,
    tip_normal: Vec3,
    model_id: &str,
    mesh: ModelMesh<'_>,
    sizing: Option<CavityBridgeSizing>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
    landing: Landing,
    far: bool,
) -> Result<CavityBridgeResult, BridgeRefusal> {
    let b_pos = landing.point;
    let (dx, dy, dz) = (
        tip_pos.x - b_pos.x,
        tip_pos.y - b_pos.y,
        tip_pos.z - b_pos.z,
    );
    let span = (dx * dx + dy * dy + dz * dz).sqrt();
    let cutoff = settings.mesh_to_mesh.stick_vs_twig_cutoff_mm;
    let read_setting = |path: SupportPlacementSettingPath| match path {
        SupportPlacementSettingPath::MeshToMeshStickVsTwigCutoffMm => Some(cutoff),
    };
    let kind = select_type_for_placement(SupportPlacementMetric::ContactSpan, span, &read_setting)
        .ok_or(BridgeRefusal::NoType)?;
    // A landing past the near cutoff on the wide search is a prop from the
    // side, which only a type that may reach sideways builds.
    if !bridge_may_land_sideways(kind, span, cutoff, far) {
        return Err(BridgeRefusal::Sideways);
    }

    let built = build_contact_bridge(
        kind,
        &ContactBridgeRequest {
            model_id,
            a_pos: tip_pos,
            a_normal: tip_normal,
            b_pos,
            b_normal: landing.normal,
            shaft_diameter_mm: sizing.map(|s| s.shaft_diameter_mm),
            tip_contact_diameter_mm: sizing.map(|s| s.tip_contact_diameter_mm),
            mesh: None,
            manual: None,
        },
        settings,
        new_id,
    )
    .ok_or(BridgeRefusal::Canted)?;
    let entity = built.entity;

    // The shaft must stay out of the model. A short bridge (a type whose
    // span rule has an upper bound) is tested with rays, as the twig builder
    // does, since the distance field reads a thin gap as material.
    let shaft = entity.segments().first();
    let start = shaft
        .and_then(|s| s.bottom_joint.as_ref())
        .map_or(b_pos, |j| j.pos);
    let end = shaft
        .and_then(|s| s.top_joint.as_ref())
        .map_or(tip_pos, |j| j.pos);
    let radius = shaft.map_or_else(
        || sizing.map_or(1.0, |s| s.shaft_diameter_mm),
        |s| s.diameter,
    ) / 2.0
        + 0.15;
    let short = get_support_type_descriptor(kind)
        .placement_rule
        .is_some_and(|rule| rule.max_mm.is_some());
    let blocked = if short {
        check_short_bridge_collision(start, end, radius, mesh.rays).hit
    } else {
        is_shaft_blocked(start, end, radius, mesh.field)
    };
    // Nor may a stick's tip cone pass through the model (#500).
    let cone_crosses = |entity: &BridgeEntity| match entity {
        BridgeEntity::Stick(stick) => [&stick.contact_cone_a, &stick.contact_cone_b]
            .into_iter()
            .any(|cone| tip_cone_crosses_model(cone, mesh.rays)),
        BridgeEntity::Twig(_) => false,
    };
    if blocked || cone_crosses(&entity) {
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
}

/// Whether the contact's normal faces into the model: from a short step out
/// along it, the way back to the contact enters through the back of the
/// surface.
fn faces_into_model(
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
