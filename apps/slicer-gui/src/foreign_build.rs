/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! What a scene from another slicer holds that Breadslicer's own engines make
//! again when it opens (#699, #700): each hollowed model hollowed by the
//! hollowing engine with its wall, drains and solid zones, then each support
//! put back where it touched the model, with its sizes, and routed by the
//! support engine. Runs on the load worker, after the models stand on their
//! plates, so the sets and hollows install as a `.bread`'s do.

use std::sync::Arc;

use rayon::prelude::*;
use sc_geom::hollow::hollow_engine::GridRequest;
use sc_geom::hollow::hollow_engine::LassoRegion;
use sc_geom::hollow::{
    HOLLOW_ENGINE_VERSION, HollowKind, HollowRequest, HollowResult, LeafFrame, start_hollow_mesh,
};
use sc_geom::supports::{
    AddOptions, BaseTip, HandShape, RestoredSet, SUPPORT_MESH_NAME, SupportRequest, SupportSession,
    SupportSize, empty_session,
};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_project_formats::{
    ForeignModel, ForeignScene, LysContact, LysSolidZone, ReadProjectWarning,
};
use sc_scene::node::{LeafPayload, NodeKind};
use sc_scene::{NodeId, Scene, SceneMetadata};

use crate::hollow_persist::RestoredHollows;
use crate::hollow_run::{HollowOptions, Hollowed, LocalDrain, solid_areas};
use crate::support_persist::RestoredSupports;

/// What making a foreign scene's supports and hollows again produced.
#[derive(Default)]
pub(crate) struct ForeignBuilt {
    pub(crate) supports: RestoredSupports,
    pub(crate) hollows: RestoredHollows,
    /// What could not be made again, when anything.
    pub(crate) note: Option<ReadProjectWarning>,
}

/// Makes again what `foreign` says each model of `scene` holds: its
/// hollowing, then its supports, made as `request` says. The models are
/// `scene`'s roots, in the rows `foreign` lists them in.
pub(crate) fn build_foreign(
    scene: &mut Scene,
    foreign: &ForeignScene,
    request: &SupportRequest,
) -> ForeignBuilt {
    let mut built = ForeignBuilt::default();
    let (mut lost_supports, mut lost_drains, mut lost_hollows) = (0, 0, 0);
    let rows: Vec<(NodeId, &ForeignModel)> =
        scene.roots().iter().copied().zip(&foreign.models).collect();

    // The hollows first: the supports are made on the model as it prints.
    for &(leaf, model) in &rows {
        let Some(hollow) = &model.hollow else {
            // Drains into a model that is not hollow drain nothing.
            lost_drains += model.drains.len();
            continue;
        };
        match hollow_again(scene, leaf, hollow.wall_mm, model) {
            Some((hollowed, missed)) => {
                lost_drains += missed;
                built.hollows.hollows.push(hollowed);
            }
            None => {
                lost_hollows += 1;
                lost_drains += model.drains.len();
            }
        }
    }

    // A session per model, on the model as it now is; each fills apart.
    let mut sessions: Vec<(NodeId, SupportSession, &[LysContact], Transform3D)> = Vec::new();
    for &(leaf, model) in &rows {
        if model.contacts.is_empty() {
            continue;
        }
        let world = world_of(scene, leaf);
        match empty_session(scene, leaf, request) {
            Ok(Some(session)) => sessions.push((leaf, session, &model.contacts, world)),
            _ => lost_supports += model.contacts.len(),
        }
    }
    let filled: Vec<(NodeId, SupportSession, usize)> = sessions
        .into_par_iter()
        .map(|(leaf, mut session, contacts, world)| {
            let lost = put_back(&mut session, contacts, &world);
            (leaf, session, lost)
        })
        .collect();
    for (model, session, lost) in filled {
        lost_supports += lost;
        let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.mesh()));
        let Ok(Ok(Some(mesh))) = drawn else {
            lost_supports += session.tips().len();
            continue;
        };
        match add_support_leaf(scene, mesh) {
            Some(leaf) => built.supports.sets.push(RestoredSet {
                leaf,
                model,
                session,
            }),
            None => lost_supports += session.tips().len(),
        }
    }

    if lost_supports + lost_drains + lost_hollows > 0 {
        built.note = Some(ReadProjectWarning::NotBroughtIn {
            supports: lost_supports,
            drains: lost_drains,
            hollows: lost_hollows,
        });
    }
    built
}

/// The world transform of root `leaf`.
fn world_of(scene: &Scene, leaf: NodeId) -> Transform3D {
    scene
        .node(leaf)
        .map_or(Transform3D::Identity, |n| n.local_transform)
}

fn affine(t: &Transform3D) -> AffineTransform3D {
    match t {
        Transform3D::Identity => AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0.0; 3],
        ),
        Transform3D::Affine(a) => *a,
    }
}

/// `p` of the model's mesh frame, in the world.
fn point_in(world: &AffineTransform3D, p: [f64; 3]) -> [f64; 3] {
    let (l, t) = (world.linear(), world.translation());
    std::array::from_fn(|i| (0..3).map(|k| l[i][k] * p[k]).sum::<f64>() + t[i])
}

/// The normal `n` of the model's mesh frame, in the world: under the inverse
/// transpose of its linear part, so a stretched model keeps its normals
/// square to its surface.
fn normal_in(world: &AffineTransform3D, n: [f64; 3]) -> [f64; 3] {
    let m = world.linear();
    // The cofactors are the inverse transpose up to the determinant, whose
    // sign keeps the normal out of the model.
    let c = |r: usize, k: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
        m[r1][k1] * m[r2][k2] - m[r1][k2] * m[r2][k1]
    };
    let det: f64 = (0..3).map(|k| m[0][k] * c(0, k)).sum();
    let out: [f64; 3] =
        std::array::from_fn(|i| (0..3).map(|k| c(i, k) * n[k]).sum::<f64>() * det.signum());
    let len = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2]).sqrt();
    if len > 1e-12 { out.map(|v| v / len) } else { n }
}

/// The sizes a support of the scene was made with, as a support placed by
/// hand takes them.
fn hand_shape(contact: &LysContact) -> HandShape {
    let s = &contact.shape;
    HandShape(SupportSize {
        id: "lys".to_owned(),
        tip_diameter_mm: s.tip_diameter,
        tip_length_mm: s.tip_length,
        penetration_mm: s.penetration,
        break_point_mm: s.break_point,
        trunk_diameter_mm: s.shaft_diameter,
        base_diameter_mm: s.base_diameter,
        base_thickness_mm: s.base_thickness,
        join_cone_mm: s.join_cone,
        base_tip: s.base_tip.map(|(length_mm, diameter_mm)| BaseTip {
            length_mm,
            diameter_mm,
        }),
    })
}

/// Puts each of `contacts` back on the model of `session`, posed by
/// `world`; how many found no way to be placed.
fn put_back(session: &mut SupportSession, contacts: &[LysContact], world: &Transform3D) -> usize {
    let world = affine(world);
    let mut lost = 0;
    for contact in contacts {
        let how = AddOptions {
            shape: Some(hand_shape(contact)),
            ..AddOptions::default()
        };
        let tip = point_in(&world, contact.point);
        let normal = normal_in(&world, contact.normal);
        let placed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut tried = session.clone();
            tried.add_contact(tip, normal, &how).map(|()| tried)
        }));
        match placed {
            Ok(Ok(done)) => *session = done,
            _ => lost += 1,
        }
    }
    lost
}

/// A root leaf showing a set's supports, drawn in the world.
fn add_support_leaf(scene: &mut Scene, mesh: IndexedMesh) -> Option<NodeId> {
    let meta = SceneMetadata {
        name: Some(SUPPORT_MESH_NAME.to_owned()),
        ..SceneMetadata::default()
    };
    let rid = scene.add_mesh_resource_arc(Arc::new(mesh));
    let id = scene
        .add_leaf(
            Some(meta),
            Transform3D::Identity,
            LeafPayload::MeshResource(rid),
        )
        .ok()?;
    scene.add_root(id).ok()?;
    Some(id)
}

/// A solid zone of the scene in the model's mesh frame: the unit cube its
/// matrix maps, a box along its own axes.
fn solid_region(zone: &LysSolidZone) -> LassoRegion {
    let m = &zone.matrix;
    let column = |c: usize| [m[0][c] as f32, m[1][c] as f32, m[2][c] as f32];
    LassoRegion {
        perspective: false,
        origin: column(3),
        u: column(0),
        v: column(1),
        w: column(2),
        polygon: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]],
        depth: Some([-0.5, 0.5]),
    }
}

/// Hollows the model `leaf` shows again, with a wall of `wall_mm` and what
/// `model` drains and keeps solid; its mesh becomes the hollow. The hollow
/// to install, and how many drains reach no cavity; `None` when it cannot
/// be hollowed (it keeps its mesh).
fn hollow_again(
    scene: &mut Scene,
    leaf: NodeId,
    wall_mm: f64,
    model: &ForeignModel,
) -> Option<(Hollowed, usize)> {
    let original = scene.leaf_mesh_arc(leaf)?.clone();
    let world = world_of(scene, leaf);
    let frame = LeafFrame::of(&world).ok()?;
    let options = HollowOptions {
        wall_mm: wall_mm as f32,
        ..HollowOptions::default()
    };
    let drains: Vec<LocalDrain> = model
        .drains
        .iter()
        .map(|d| LocalDrain {
            center: d.point.map(|c| c as f32),
            // Into the model, against the normal out of it.
            direction: d.normal.map(|c| -c as f32),
            radius_mm: (d.diameter / 2.0) as f32,
            ..LocalDrain::default()
        })
        .collect();
    let solids: Vec<LassoRegion> = model.solid_zones.iter().map(solid_region).collect();
    let request = HollowRequest {
        kind: HollowKind::Apply,
        grid: GridRequest {
            voxel_size_mm: options.voxel_mm,
            ..GridRequest::default()
        },
        settings: options.settings(),
        drains: drains.iter().map(|d| d.to_world(&world)).collect(),
        solid_areas: solid_areas(&solids, &world),
        ..HollowRequest::default()
    };
    let outcome = start_hollow_mesh(original.clone(), &world, request)
        .ok()?
        .join()
        .ok()?;
    if outcome.stats.removed_voxels == 0 {
        return None;
    }
    let HollowResult::Apply {
        mesh, drains: done, ..
    } = outcome.result
    else {
        return None;
    };
    let kept: Vec<LocalDrain> = drains
        .iter()
        .zip(&done)
        .filter(|(_, report)| report.reaches_cavity)
        .map(|(drain, _)| *drain)
        .collect();
    let missed = drains.len() - kept.len();
    let hollow = Arc::new(mesh);
    match scene.node(leaf).map(|n| &n.kind) {
        Some(NodeKind::Leaf(LeafPayload::Mesh(before))) => {
            let before = before.clone();
            scene
                .replace_inline_leaf_mesh(leaf, &before, hollow.clone())
                .ok()?;
        }
        _ => return None,
    }
    Some((
        Hollowed {
            hollow,
            original,
            drains: kept,
            options,
            plugs: Vec::new(),
            solids,
            engine: Some(HOLLOW_ENGINE_VERSION),
            stretch: Some(frame.stretch_rows()),
            drilled: false,
        },
        missed,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stretched_model_keeps_its_normals_square_to_its_surface() {
        // Stretched ×2 along X: a slanted face's normal turns toward X less.
        let world = AffineTransform3D::from_linear_and_translation(
            [[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [5.0, 0.0, 0.0],
        );
        let n = normal_in(&world, [1.0, 0.0, 1.0]);
        // The face x + z = 0 becomes x / 2 + z = 0: normal (1/2, 0, 1).
        let want = [0.5 / 1.25f64.sqrt(), 0.0, 1.0 / 1.25f64.sqrt()];
        for k in 0..3 {
            assert!((n[k] - want[k]).abs() < 1e-9, "{n:?}");
        }
        assert_eq!(point_in(&world, [1.0, 2.0, 3.0]), [7.0, 2.0, 3.0]);
        // A mirror keeps the normal out of the model.
        let mirror = AffineTransform3D::from_linear_and_translation(
            [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0.0, 0.0, 0.0],
        );
        assert_eq!(normal_in(&mirror, [1.0, 0.0, 0.0]), [-1.0, 0.0, 0.0]);
    }

    /// The fixture's box opened as the app opens it: hollowed with its
    /// drain and solid zone, its support put back where the file says.
    #[test]
    fn a_lychee_scene_opens_hollowed_and_supported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("box.lys");
        std::fs::write(&path, bread_lys::fixture::box_on_second_plate()).expect("write");
        let loaded = sc_project_formats::load_scene(&path, None, |_| {}).expect("reads");
        let volume = sc_print_primitives::BuildVolume::new(128.0, 81.0, 160.0).unwrap();
        let opened = crate::mesh_loader::opened_scene(loaded, &path, volume).expect("opens");
        let (supports, hollows, notes) = opened.built_for_tests();
        // Everything was made again (the resin is one we lack: a note).
        assert!(
            !notes
                .iter()
                .any(|n| matches!(n, ReadProjectWarning::NotBroughtIn { .. })),
            "{notes:?}"
        );

        // One hollow: a thinner model, its drain kept, its zone solid.
        assert_eq!(hollows.hollows.len(), 1);
        let hollow = &hollows.hollows[0];
        assert!(hollow.hollow.faces().len() > hollow.original.faces().len());
        assert_eq!(hollow.options.wall_mm, 0.5);
        assert_eq!(hollow.drains.len(), 1);
        assert_eq!(hollow.drains[0].radius_mm, 0.5);
        assert_eq!(hollow.solids.len(), 1);
        assert_eq!(hollow.solids[0].depth, Some([-0.5, 0.5]));

        // One set on the box, its tip where the file says: under the box's
        // middle, on its bottom 3 mm up (its plate's frame, then the plate's
        // place in the row of plates).
        assert_eq!(supports.sets.len(), 1);
        let set = &supports.sets[0];
        let model = opened.scene.roots()[0];
        assert_eq!(set.model, model);
        let world = opened.scene.node(model).unwrap().local_transform;
        let want = world
            .apply_to_vertex(sc_mesh_core::Vertex::new([0.0, 0.0, -3.0]))
            .coords()
            .map(f64::from);
        let tips: Vec<[f64; 3]> = set
            .session
            .pieces()
            .into_iter()
            .flat_map(|p| p.contacts)
            .map(|c| c.surface)
            .collect();
        assert_eq!(tips.len(), 1, "{tips:?}");
        let off = (0..3)
            .map(|k| (tips[0][k] - want[k]).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(off < 0.01, "{:?} against {want:?}", tips[0]);
        assert!((want[2] - 3.0).abs() < 1e-4, "{want:?}");
        // It is a support placed by hand: editable, kept through a run
        // made again.
        assert!(set.session.hand_edited());
    }

    #[test]
    fn a_solid_zone_is_the_box_its_matrix_maps() {
        let zone = LysSolidZone {
            matrix: [
                [2.0, 0.0, 0.0, 1.0],
                [0.0, 4.0, 0.0, 2.0],
                [0.0, 0.0, 6.0, 3.0],
            ],
        };
        let region = solid_region(&zone).prepared().expect("a box");
        use sc_geom::hollow::hollow_engine::mesh::Vec3;
        assert!(region.contains(Vec3::new(1.9, 3.9, 5.9)));
        assert!(!region.contains(Vec3::new(1.0, 2.0, 6.1)));
        assert!(!region.contains(Vec3::new(2.1, 2.0, 3.0)));
    }
}
