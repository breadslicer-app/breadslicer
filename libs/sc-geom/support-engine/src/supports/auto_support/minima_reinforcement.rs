//! DragonFruit `src/supports/autoSupport/minimaReinforcement.ts`.
//!
//! Radial reinforcement around mesh minima. A minima is the first point of a
//! section, so its own tip support holds a POINT; this pass spreads the
//! contact with a ring of contacts around it at `MINIMA_RING_RADIUS_MM`, each
//! landing on the feature's own flank. Only minima the voxel mask did NOT see
//! (`class === 'minimaOnly'`) are reinforced. A contact exists only where the
//! ring's upward ray finds a flank between `MINIMA_RING_MIN_RISE_MM` and
//! `radius · tan(selfSupportAngle)` above the minima.

use serde::{Deserialize, Serialize};

use super::constants::{MINIMA_RING_COUNT, MINIMA_RING_MIN_RISE_MM, MINIMA_RING_RADIUS_MM};
use super::settings::AutoSupportSettings;
use crate::three::Ray;
use crate::three::math_utils::{js_cos, js_sin, js_tan};
use crate::three::vector3::Vec3;
use crate::three_mesh_bvh::{BvhMesh, Side};
use crate::volume_analysis::islands::types::{DetectedIsland, IslandClass, IslandSource};

/// One reinforcement contact and the minima it rings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinimaReinforcementPoint {
    /// The minima island being reinforced.
    pub island_id: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// The ring contacts for every unseen minima. `side` is the mesh material's
/// side, which the raycasts honour.
pub fn compute_minima_reinforcement_points(
    islands: &[DetectedIsland],
    mesh: BvhMesh<'_>,
    side: Side,
    auto: &AutoSupportSettings,
) -> Vec<MinimaReinforcementPoint> {
    let self_support_angle_deg = auto.overhang_self_support_angle_deg;
    let max_rise_mm =
        MINIMA_RING_RADIUS_MM * js_tan((self_support_angle_deg * std::f64::consts::PI) / 180.0);

    let mut points = Vec::new();
    for island in islands {
        if island.source != IslandSource::Minima || island.class != Some(IslandClass::MinimaOnly) {
            continue;
        }
        let Vec3 {
            x: cx,
            y: cy,
            z: cz,
        } = island.contact;
        for i in 0..MINIMA_RING_COUNT {
            let angle = (i as f64 / MINIMA_RING_COUNT as f64) * std::f64::consts::PI * 2.0;
            let x = cx + js_cos(angle) * MINIMA_RING_RADIUS_MM;
            let y = cy + js_sin(angle) * MINIMA_RING_RADIUS_MM;
            // Rising from the minima's own plane: every hit is at or above it,
            // and `far` bounds the rise, so the band needs only its floor.
            let ray = Ray {
                origin: Vec3::new(x, y, cz),
                direction: Vec3::new(0.0, 0.0, 1.0),
            };
            let hit = mesh
                .intersect_object(&ray, side, 0.0, max_rise_mm, false)
                .into_iter()
                .find(|h| h.point.z - cz >= MINIMA_RING_MIN_RISE_MM);
            if let Some(hit) = hit {
                points.push(MinimaReinforcementPoint {
                    island_id: island.id.clone(),
                    x,
                    y,
                    z: hit.point.z,
                });
            }
        }
    }
    points
}
