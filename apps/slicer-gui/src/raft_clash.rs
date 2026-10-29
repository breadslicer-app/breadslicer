/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! A model standing lower than the top of the supports' raft (#131).
//!
//! The raft (its plate and crenelated wall) takes the first layers above the
//! build plate, and it is cleared around a model that stands in it: the
//! model's first layers share the raft's, and the supports, which start on
//! the raft, never reach what the model overhangs there. The Supports tab
//! warns on Generate and offers to lift the models clear of the raft or to
//! turn the raft off; the Review card offers the same for a slice.

use sc_geom::df_supports::{DfSupportOptions, recommended_lift_mm};
use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;
use sc_scene_edit::SceneEdit;

use crate::viewport::numeric_transform::{self, NumericEdit};

/// What the warning says and the lift it offers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RaftClash {
    /// The layers the raft takes, from the plate up.
    pub layers: usize,
    /// The top of the raft above the plate: a model lower than this is in
    /// it.
    pub raft_top_mm: f64,
    /// Where a model's lowest point should stand: the generator's default
    /// support lift (or a higher configured one), which leaves the supports
    /// room under the model, never below the raft's last layer.
    pub lift_to_mm: f64,
}

/// How far below the raft's top a model may reach and still clear it: only
/// absorbs rounding in a typed lift.
const TOLERANCE_MM: f64 = 1e-6;

/// The clash between a raft built with `options`, sliced in layers of
/// `layer_height_mm`, and models whose lowest point is `lowest_model_mm`
/// above the plate (`None` without a model). `None` when there is no raft
/// or the models stand above it.
pub(crate) fn raft_clash(
    options: &DfSupportOptions,
    layer_height_mm: f64,
    lowest_model_mm: Option<f64>,
) -> Option<RaftClash> {
    let top = options.raft_top_mm()?;
    let lowest = lowest_model_mm?;
    let layers = options.raft_layers(layer_height_mm);
    if layers == 0 || lowest >= top - TOLERANCE_MM {
        return None;
    }
    Some(RaftClash {
        layers,
        raft_top_mm: top,
        lift_to_mm: recommended_lift_mm(None).max(layers as f64 * layer_height_mm),
    })
}

/// Every model leaf of `spatial` (generated supports left out) with its
/// lowest point above the plate.
pub(crate) fn model_bottoms(spatial: &SceneSpatial) -> Vec<(NodeId, f64)> {
    let scene = spatial.graph();
    (0..)
        .map_while(|i| spatial.leaf_id_at_index(i))
        .filter_map(|id| {
            let mesh = scene.leaf_mesh_arc(id)?;
            if sc_geom::df_supports::is_support_mesh(mesh) {
                return None;
            }
            let query = spatial.leaf(id)?;
            Some((id, query.minimum_world_z(query.world_transform())?))
        })
        .collect()
}

/// The lowest point of every model of `spatial` (only those `only` names,
/// when given), or `None` without a model.
pub(crate) fn lowest_model_mm(spatial: &SceneSpatial, only: Option<&[NodeId]>) -> Option<f64> {
    model_bottoms(spatial)
        .into_iter()
        .filter(|(id, _)| only.is_none_or(|only| only.contains(id)))
        .map(|(_, z)| z)
        .reduce(f64::min)
}

/// The edit that lifts every model of `spatial` (only those `only` names,
/// when given) lower than `below_mm` (in the raft) so its lowest point
/// stands at `lift_to_mm`, as one undoable step; `None` when no model is
/// that low.
pub(crate) fn lift_edit(
    spatial: &SceneSpatial,
    below_mm: f64,
    lift_to_mm: f64,
    only: Option<&[NodeId]>,
) -> Option<SceneEdit> {
    let below_mm = below_mm.min(lift_to_mm);
    let mut edits: Vec<SceneEdit> = model_bottoms(spatial)
        .into_iter()
        .filter(|(id, _)| only.is_none_or(|only| only.contains(id)))
        .filter(|(_, z)| *z < below_mm - TOLERANCE_MM)
        .filter_map(|(id, _)| {
            let query = spatial.leaf(id)?;
            let before = query.local_transform();
            let after = numeric_transform::apply(&query, NumericEdit::Lift(lift_to_mm))?;
            Some(SceneEdit::SetLocalTransform {
                node_id: id,
                before,
                after,
            })
        })
        .collect();
    match edits.len() {
        0 => None,
        1 => edits.pop(),
        _ => Some(SceneEdit::Compound(edits)),
    }
}

#[cfg(test)]
mod raft_clash_tests {
    use std::sync::Arc;

    use sc_geom::df_supports::{DEFAULT_SUPPORT_LIFT_MM, RaftMode};
    use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
    use sc_scene::Scene;
    use sc_scene::{LeafPayload, SceneMetadata};

    use super::*;

    #[test]
    fn a_model_on_the_plate_clashes_with_the_default_raft() {
        // Plate 0.5 mm plus wall 0.35 mm: 17 layers of 0.05 mm, 9 of 0.1 mm.
        let options = DfSupportOptions::default();
        let clash = raft_clash(&options, 0.05, Some(0.0)).expect("in the raft");
        assert_eq!(clash.layers, 17);
        assert!((clash.raft_top_mm - 0.85).abs() < 1e-9);
        // Not just above the raft: the default support lift, room for the
        // supports under the model.
        assert_eq!(clash.lift_to_mm, DEFAULT_SUPPORT_LIFT_MM);
        let coarse = raft_clash(&options, 0.1, Some(0.3)).expect("still in the raft");
        assert_eq!(coarse.layers, 9);
        assert_eq!(coarse.lift_to_mm, DEFAULT_SUPPORT_LIFT_MM);
        // Above the raft but below the support lift: no warning.
        assert_eq!(raft_clash(&options, 0.05, Some(2.0)), None);
    }

    #[test]
    fn a_model_above_the_raft_or_no_raft_is_clear() {
        let options = DfSupportOptions::default();
        assert_eq!(raft_clash(&options, 0.05, Some(0.85)), None);
        assert_eq!(raft_clash(&options, 0.05, Some(5.0)), None);
        assert_eq!(raft_clash(&options, 0.05, None), None);
        let off = DfSupportOptions {
            raft: RaftMode::Off,
            ..Default::default()
        };
        assert_eq!(raft_clash(&off, 0.05, Some(0.0)), None);
        let thin = DfSupportOptions {
            raft_wall: false,
            ..Default::default()
        };
        assert_eq!(raft_clash(&thin, 0.05, Some(0.6)), None);
        assert_eq!(
            raft_clash(&thin, 0.05, Some(0.4)).map(|c| c.layers),
            Some(10)
        );
    }

    /// A unit cube whose bottom stands `bottom` above the plate.
    fn add_cube(scene: &mut Scene, name: &str, x: f64, bottom: f64) {
        let leaf = scene
            .add_leaf(
                Some(SceneMetadata {
                    name: Some(name.to_owned()),
                    ..Default::default()
                }),
                Transform3D::Affine(AffineTransform3D::from_translation([x, 0.0, bottom + 0.5])),
                LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
            )
            .unwrap();
        scene.add_root(leaf).unwrap();
    }

    #[test]
    fn the_lift_raises_only_the_models_in_the_raft_in_one_edit() {
        let mut scene = Scene::new();
        add_cube(&mut scene, "Low", 0.0, 0.0);
        add_cube(&mut scene, "Lower", 3.0, 0.2);
        add_cube(&mut scene, "High", 6.0, 4.0);
        let mut spatial = SceneSpatial::from_graph(scene);
        let lowest = lowest_model_mm(&spatial, None).expect("three models");
        assert!(lowest.abs() < 1e-6, "{lowest}");
        let edit = lift_edit(&spatial, 0.85, 5.0, None).expect("models to lift");
        let SceneEdit::Compound(edits) = &edit else {
            panic!("two models lifted in one step: {edit:?}");
        };
        assert_eq!(edits.len(), 2);
        spatial.apply_edit(edit).unwrap();
        let bottoms: Vec<f64> = model_bottoms(&spatial)
            .into_iter()
            .map(|(_, z)| z)
            .collect();
        assert_eq!(bottoms.len(), 3);
        // The two in the raft rise to the lift; the one above it stays.
        assert!((bottoms[0] - 5.0).abs() < 1e-5, "{bottoms:?}");
        assert!((bottoms[1] - 5.0).abs() < 1e-5, "{bottoms:?}");
        assert!((bottoms[2] - 4.0).abs() < 1e-5, "{bottoms:?}");
        assert!(
            lift_edit(&spatial, 0.85, 5.0, None).is_none(),
            "nothing left to lift"
        );
    }
}
