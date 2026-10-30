/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The material's scale compensation (#227), applied to the scene a slice
//! is cut from.
//!
//! Each model is scaled about the centre of its footprint on the plate, in X
//! and Y, and about the plate in Z: `x' = cx + (x − cx)·(1 + sx)`, the same
//! in Y, and `z' = z·(1 + sz)`. Each support set is scaled with the model it
//! stands on, about that model's centre, so its tips stay where they touch
//! the model.
//!
//! The geometry is scaled before slicing, not the contours after it: a
//! contour stack cannot be stretched in Z without cutting the layers again,
//! and every consumer of the slice (the review, the layer preview, the
//! printer file) then reads the same scaled layers.

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_print_primitives::ScaleCompensation;
use sc_scene::Scene;
use sc_scene::node::NodeId;
use sc_sl1cer::sliceable_leaves;

/// The support leaves of a scene, each with the model it stands on when the
/// caller knows it. Every sliceable leaf not listed is a model.
///
/// A support leaf with no model, or with one that is not a model leaf of the
/// scene, goes with the model whose footprint on the plate it overlaps most,
/// or else with the nearest one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SupportOwners {
    owners: Vec<(NodeId, Option<NodeId>)>,
}

impl SupportOwners {
    /// No support leaves: every leaf is a model.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark `support` as a support leaf standing on `model`, or on a model
    /// still to be found when `None`.
    pub fn push(&mut self, support: NodeId, model: Option<NodeId>) {
        self.owners.push((support, model));
    }

    fn get(&self, leaf: NodeId) -> Option<Option<NodeId>> {
        self.owners
            .iter()
            .find(|(support, _)| *support == leaf)
            .map(|(_, model)| *model)
    }
}

impl FromIterator<(NodeId, Option<NodeId>)> for SupportOwners {
    fn from_iter<I: IntoIterator<Item = (NodeId, Option<NodeId>)>>(iter: I) -> Self {
        Self {
            owners: iter.into_iter().collect(),
        }
    }
}

/// The factor each axis is multiplied by: `1 + pct / 100`.
pub fn scale_factors(scale: ScaleCompensation) -> [f64; 3] {
    [scale.x, scale.y, scale.z].map(|pct| 1.0 + pct.value() / 100.0)
}

/// Whether `scale` changes no dimension, so the scene is sliced as it is.
pub(crate) fn is_identity(scale: ScaleCompensation) -> bool {
    [scale.x, scale.y, scale.z]
        .iter()
        .all(|pct| pct.value() == 0.0)
}

/// `scene` with every model and its supports scaled by `scale`, about the
/// model's footprint centre as `supports` pairs them. `None` when `scale`
/// changes nothing, or when the scene has no sliceable leaves to scale (the
/// engine then reports why).
pub(crate) fn compensated_scene(
    scene: &Scene,
    scale: ScaleCompensation,
    supports: &SupportOwners,
) -> Option<Scene> {
    if is_identity(scale) {
        return None;
    }
    let leaves = sliceable_leaves(scene).ok()?;
    let pivots = pivots(scene, &leaves, supports);
    scaled_about(scene, &leaves, scale_factors(scale), &pivots)
}

/// `scene` with every leaf scaled in Z about the plate, and in X and Y about
/// the origin: the heights a [`compensated_scene`] has, whatever its pivots,
/// without measuring any footprint. `None` as for [`compensated_scene`].
pub(crate) fn compensated_heights(scene: &Scene, scale: ScaleCompensation) -> Option<Scene> {
    if is_identity(scale) {
        return None;
    }
    let leaves = sliceable_leaves(scene).ok()?;
    let pivots = vec![[0.0, 0.0]; leaves.len()];
    scaled_about(scene, &leaves, scale_factors(scale), &pivots)
}

/// Scale each of `leaves` about its pivot in X and Y and about the plate in
/// Z, composing the scale onto its world transform.
fn scaled_about(
    scene: &Scene,
    leaves: &[NodeId],
    factors: [f64; 3],
    pivots: &[[f64; 2]],
) -> Option<Scene> {
    let mut scaled = scene.clone();
    for (&leaf, &[cx, cy]) in leaves.iter().zip(pivots) {
        // x' = cx + (x − cx)·f = f·x + cx·(1 − f); Z has no translation, so
        // a leaf's heights do not depend on its pivot.
        let scale = Transform3D::Affine(
            AffineTransform3D::from_translation([
                cx * (1.0 - factors[0]),
                cy * (1.0 - factors[1]),
                0.0,
            ])
            .compose(&AffineTransform3D::from_non_uniform_scale(factors)),
        );
        let local = scene.node(leaf)?.local_transform;
        let local = match scene.parent_of(leaf) {
            None => scale.compose(&local),
            // The parent's frame P stays: P⁻¹ · S · P · L puts S in world
            // space.
            Some(parent) => {
                let parent = scene.world_transform_of(parent).ok()?;
                parent
                    .inverse()
                    .ok()?
                    .compose(&scale)
                    .compose(&parent)
                    .compose(&local)
            }
        };
        scaled.set_local_transform(leaf, local).ok()?;
    }
    Some(scaled)
}

/// A leaf's footprint on the plate: `[min_x, min_y, max_x, max_y]` in world
/// millimetres. `None` for a leaf with no vertices.
type Footprint = [f64; 4];

fn footprint(scene: &Scene, leaf: NodeId) -> Option<Footprint> {
    let world = scene.world_transform_of(leaf).ok()?;
    let mesh = scene.mesh_at_leaf(leaf).ok()?;
    let mut bounds: Option<Footprint> = None;
    for &vertex in mesh.vertices() {
        let [x, y, _] = world.apply_to_vertex(vertex).coords();
        let (x, y) = (f64::from(x), f64::from(y));
        bounds = Some(match bounds {
            None => [x, y, x, y],
            Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
        });
    }
    bounds
}

fn centre(footprint: Footprint) -> [f64; 2] {
    let [x0, y0, x1, y1] = footprint;
    [(x0 + x1) / 2.0, (y0 + y1) / 2.0]
}

fn overlap(a: Footprint, b: Footprint) -> f64 {
    let width = a[2].min(b[2]) - a[0].max(b[0]);
    let depth = a[3].min(b[3]) - a[1].max(b[1]);
    width.max(0.0) * depth.max(0.0)
}

fn area(footprint: Footprint) -> f64 {
    (footprint[2] - footprint[0]) * (footprint[3] - footprint[1])
}

/// The pivot of each of `leaves`: a model's own footprint centre, and a
/// support leaf's model's.
fn pivots(scene: &Scene, leaves: &[NodeId], supports: &SupportOwners) -> Vec<[f64; 2]> {
    let footprints: Vec<Option<Footprint>> =
        leaves.iter().map(|&leaf| footprint(scene, leaf)).collect();
    let models: Vec<(NodeId, Footprint)> = leaves
        .iter()
        .zip(&footprints)
        .filter(|(leaf, _)| supports.get(**leaf).is_none())
        .filter_map(|(&leaf, footprint)| Some((leaf, (*footprint)?)))
        .collect();
    leaves
        .iter()
        .zip(&footprints)
        .map(|(&leaf, &own)| {
            let Some(own) = own else {
                return [0.0, 0.0];
            };
            let Some(owner) = supports.get(leaf) else {
                return centre(own);
            };
            let named = owner.and_then(|model| models.iter().find(|(id, _)| *id == model));
            named
                .or_else(|| best_overlap(own, &models))
                .map_or_else(|| centre(own), |(_, model)| centre(*model))
        })
        .collect()
}

/// The model whose footprint covers the largest share of the union with
/// `support`'s; with no overlap at all, the one whose centre is nearest.
fn best_overlap(
    support: Footprint,
    models: &[(NodeId, Footprint)],
) -> Option<&(NodeId, Footprint)> {
    let share = |model: &Footprint| {
        let common = overlap(support, *model);
        let union = area(support) + area(*model) - common;
        if union > 0.0 { common / union } else { 0.0 }
    };
    let distance = |model: &Footprint| {
        let [sx, sy] = centre(support);
        let [mx, my] = centre(*model);
        (sx - mx).hypot(sy - my)
    };
    let best = models
        .iter()
        .max_by(|a, b| share(&a.1).total_cmp(&share(&b.1)))?;
    if share(&best.1) > 0.0 {
        Some(best)
    } else {
        models
            .iter()
            .min_by(|a, b| distance(&a.1).total_cmp(&distance(&b.1)))
    }
}
