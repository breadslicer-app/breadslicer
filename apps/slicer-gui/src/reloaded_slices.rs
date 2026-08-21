/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Revalidation of a reopened project's persisted slices.
//!
//! A saved project carries its merged slice geometry plus a validity
//! token fingerprinting the scene geometry and the resolved layer
//! schedule. On load the slices are trusted only when that token still
//! matches: the profiles are re-resolved from the installed library and
//! the fingerprint recomputed against the reopened scene, so a profile
//! or geometry change since the save discards the stale slices and the
//! scene opens slice-less. Both steps (disk profile resolution and the
//! geometry hash) run on the load worker, off the event loop.
//!
//! The revalidated slices ride to the UI thread and become the held
//! [`SliceResult`] at install time, when the scene-geometry marker is
//! known.

use sc_print_glue::{PersistedSliceValidity, validate_persisted_slices};
use sc_print_primitives::{AaLevel, MaterialId, PositiveFiniteMm, PrinterId};
use sc_project_formats::ProjectPayload;
use sc_scene::Scene;
use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::profile::LayerSchedule;

use crate::scene_document::SceneRevision;
use crate::slice_projection::resolve_current_schedule;
use crate::slice_result::{SliceParams, SliceResult};

/// A project's persisted slices that passed revalidation on load: the
/// reloaded contour stack plus the schedule and the inputs they were
/// validated against. Built on the worker and turned into a held
/// [`SliceResult`] at install time.
pub(crate) struct ReloadedSlices {
    contours: ContourStack,
    schedule: LayerSchedule,
    token: String,
    printer: PrinterId,
    resin: MaterialId,
    first_layer_height: Option<PositiveFiniteMm>,
}

impl ReloadedSlices {
    /// Build the held slice result, stamping the scene-geometry marker
    /// captured after the fresh install (`SceneRevision(None)` on a clean
    /// load), so the reloaded slice reads as in sync and re-persists
    /// without a re-slice. The anti-aliasing level is not persisted and
    /// does not feed the token, so it defaults to `Off`.
    pub(crate) fn into_slice_result(self, geometry: SceneRevision) -> SliceResult {
        SliceResult {
            merged: self.contours,
            schedule: self.schedule,
            token: self.token,
            params: SliceParams {
                printer: self.printer,
                resin: self.resin,
                first_layer_height: self.first_layer_height,
                aa_level: AaLevel::Off,
                geometry,
            },
        }
    }
}

/// Revalidate a project's persisted slices against the reopened scene,
/// returning them only when they are still in sync. Resolves the layer
/// schedule from the installed profiles the same way the save path does,
/// then recomputes the geometry fingerprint and compares it to the
/// stored token. Any missing input (no slices, no token, no printer or
/// resin, or a schedule that no longer resolves) or a token mismatch
/// yields `None`, and the scene loads slice-less.
pub(crate) fn revalidate_persisted_slices(
    payload: Option<&ProjectPayload>,
    scene: &Scene,
    slices: Option<ContourStack>,
    fingerprint: Option<String>,
) -> Option<ReloadedSlices> {
    let contours = slices?;
    let token = fingerprint?;
    let payload = payload?;
    let printer = payload.printer.clone()?;
    let resin = payload.resin.clone()?;
    let first_layer_height = payload.first_layer_height;

    let schedule = resolve_current_schedule(&printer, &resin, first_layer_height, scene).ok()?;
    if !slices_in_sync(&token, scene, &schedule) {
        return None;
    }
    Some(ReloadedSlices {
        contours,
        schedule,
        token,
        printer,
        resin,
        first_layer_height,
    })
}

/// Whether the stored token still matches the fingerprint of the scene
/// under the resolved schedule.
fn slices_in_sync(token: &str, scene: &Scene, schedule: &LayerSchedule) -> bool {
    matches!(
        validate_persisted_slices(token, scene, schedule),
        PersistedSliceValidity::InSync
    )
}

#[cfg(test)]
mod reloaded_slices_tests {
    use super::*;

    use std::io::Cursor;
    use std::path::Path;
    use std::sync::Arc;

    use sc_mesh_core::indexed_mesh::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::transform::Transform3D;
    use sc_print_glue::slice_input_fingerprint;
    use sc_project_formats::{
        EmbedProfiles, ProjectFileType, infer_project_file_type_from_extension, read_project,
        write_project,
    };
    use sc_scene::node::LeafPayload;

    fn cube_mesh() -> IndexedMesh {
        let v = |x, y, z| Vertex::new([x, y, z]);
        let vertices = vec![
            v(-1.0, -1.0, 0.0),
            v(1.0, -1.0, 0.0),
            v(1.0, 1.0, 0.0),
            v(-1.0, 1.0, 0.0),
            v(-1.0, -1.0, 2.0),
            v(1.0, -1.0, 2.0),
            v(1.0, 1.0, 2.0),
            v(-1.0, 1.0, 2.0),
        ];
        let n = Normal::new([0.0, 0.0, 1.0]);
        let faces = vec![
            IndexedTriangle::from_raw_indices(n, [0, 1, 2]),
            IndexedTriangle::from_raw_indices(n, [0, 2, 3]),
        ];
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    fn cube_scene() -> Scene {
        let mut scene = Scene::new();
        let root = scene.add_group(None, Transform3D::Identity);
        scene.add_root(root).expect("group as root");
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(cube_mesh())),
            )
            .expect("add leaf");
        scene.add_child(root, leaf).expect("attach leaf");
        scene
    }

    fn schedule() -> LayerSchedule {
        LayerSchedule::uniform(0.05, 0.05, 40).expect("a positive, non-empty schedule")
    }

    fn token_for(scene: &Scene, schedule: &LayerSchedule) -> String {
        slice_input_fingerprint(scene, schedule)
            .expect("the scene has sliceable geometry")
            .to_token()
    }

    fn three_mf() -> ProjectFileType {
        infer_project_file_type_from_extension(Path::new("plate.3mf"))
            .expect("the 3mf extension is a project format")
    }

    fn empty_payload() -> ProjectPayload {
        ProjectPayload {
            printer: None,
            resin: None,
            first_layer_height: None,
            embedded: Vec::new(),
        }
    }

    #[test]
    fn the_fingerprint_survives_a_project_write_read_round_trip() {
        // The whole feature rests on this: if the scene fingerprint did
        // not survive a save/reopen, every reopened project would read
        // as stale and never show its slices.
        let scene = cube_scene();
        let schedule = schedule();
        let token = token_for(&scene, &schedule);

        let mut bytes = Vec::new();
        write_project(
            &scene,
            &empty_payload(),
            EmbedProfiles::No,
            None,
            Some(&token),
            three_mf(),
            &mut bytes,
        )
        .expect("the project writes cleanly");

        let loaded = read_project(&mut Cursor::new(bytes), three_mf()).expect("the project reads");
        assert!(
            slices_in_sync(&token, &loaded.scene, &schedule),
            "the fingerprint must match after a project round trip"
        );
        assert_eq!(loaded.slice_fingerprint.as_deref(), Some(token.as_str()));
    }

    #[test]
    fn a_matching_token_is_in_sync_and_a_wrong_one_is_not() {
        let scene = cube_scene();
        let schedule = schedule();
        let token = token_for(&scene, &schedule);

        assert!(slices_in_sync(&token, &scene, &schedule));
        assert!(!slices_in_sync("not-the-real-token", &scene, &schedule));
    }

    #[test]
    fn a_changed_schedule_is_not_in_sync() {
        // The token covers the resolved schedule by value, so slicing at
        // a different layer height must invalidate it.
        let scene = cube_scene();
        let token = token_for(&scene, &schedule());
        let other = LayerSchedule::uniform(0.1, 0.05, 20).expect("a valid schedule");
        assert!(!slices_in_sync(&token, &scene, &other));
    }

    #[test]
    fn into_slice_result_stamps_the_geometry_and_defaults_aa() {
        let scene = cube_scene();
        let schedule = schedule();
        let token = token_for(&scene, &schedule);
        let printer = PrinterId::new("elegoo-saturn-4-ultra").expect("valid id");
        let resin = MaterialId::new("standard-405").expect("valid id");
        let first_layer_height = Some(PositiveFiniteMm::new(0.05).expect("valid length"));

        let reloaded = ReloadedSlices {
            contours: ContourStack::new_unchecked(Vec::new()),
            schedule,
            token: token.clone(),
            printer: printer.clone(),
            resin: resin.clone(),
            first_layer_height,
        };
        let geometry = SceneRevision::from_top_edit_id(None);
        let result = reloaded.into_slice_result(geometry);

        assert_eq!(result.token, token);
        assert_eq!(result.params.printer, printer);
        assert_eq!(result.params.resin, resin);
        assert_eq!(result.params.first_layer_height, first_layer_height);
        assert_eq!(result.params.aa_level, AaLevel::Off);
        assert_eq!(result.params.geometry, geometry);
    }

    #[test]
    fn missing_slices_or_token_or_profiles_yield_no_reloaded_slices() {
        let scene = cube_scene();
        let stack = || ContourStack::new_unchecked(Vec::new());
        let token = || Some(token_for(&scene, &schedule()));

        // No persisted geometry, no token: nothing to revalidate.
        assert!(
            revalidate_persisted_slices(Some(&empty_payload()), &scene, None, token()).is_none()
        );
        assert!(
            revalidate_persisted_slices(Some(&empty_payload()), &scene, Some(stack()), None)
                .is_none()
        );
        // A payload without a printer or resin cannot resolve a schedule.
        assert!(
            revalidate_persisted_slices(Some(&empty_payload()), &scene, Some(stack()), token())
                .is_none()
        );
        assert!(revalidate_persisted_slices(None, &scene, Some(stack()), token()).is_none());
    }

    #[test]
    fn a_reopened_project_revalidates_slices_saved_on_a_placed_scene() {
        // Reproduces the reopen scenario: a mesh placed by a non-identity
        // affine transform (a fit scale plus recentre), sliced on the
        // bundled default profiles, saved with real slice geometry, then
        // reopened. The full reopen path (read back the standalone slice
        // part, re-resolve the schedule, recompute the fingerprint) must
        // accept the slices.
        //
        // The slice-time scene is built the way the plain-mesh loader does:
        // the mesh leaf is the scene's direct root, with the plate placement
        // written onto it. Reopening from 3MF wraps that leaf under a fresh
        // identity group, so the saved and reopened scenes place the same
        // geometry under different nesting. The fingerprint must ignore that
        // nesting, or the reopened slices are wrongly rejected as stale.
        use sc_mesh_core::transform::AffineTransform3D;
        use sc_project_formats::PlateSlices;
        use sc_sl1cer::contour::{ClosedRing, ContourLayer, Polygon};
        use sc_sl1cer::geom2d::Point2D;

        let printer = PrinterId::new("elegoo-mars-5-goo").expect("valid id");
        let resin = MaterialId::new("sirayatech-craft-ultra-clear-saturn4").expect("valid id");

        let placement = Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
            [
                [0.6173767447471619, 0.0, 0.0],
                [0.0, 0.6173767447471619, 0.0],
                [0.0, 0.0, 0.6173767447471619],
            ],
            [
                -67.91707062404612,
                -67.90612409423966,
                0.0021667425876330526,
            ],
        ));
        let mut scene = Scene::new();
        let leaf = scene
            .add_leaf(None, placement, LeafPayload::Mesh(Arc::new(cube_mesh())))
            .expect("add leaf");
        scene.add_root(leaf).expect("leaf becomes the scene root");

        let schedule = resolve_current_schedule(&printer, &resin, None, &scene)
            .expect("the bundled default profiles resolve a schedule");
        let token = slice_input_fingerprint(&scene, &schedule)
            .expect("the scene has sliceable geometry")
            .to_token();

        let slab_tops: Vec<f64> = schedule.iter_slab_tops().collect();
        let square = Polygon::new(
            ClosedRing::new(vec![
                Point2D::new(-1.0, -1.0),
                Point2D::new(1.0, -1.0),
                Point2D::new(1.0, 1.0),
                Point2D::new(-1.0, 1.0),
            ]),
            vec![],
        )
        .expect("a CCW square is a valid polygon");
        let layers: Vec<ContourLayer> = slab_tops
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let polys = if i == 0 { vec![] } else { vec![square.clone()] };
                ContourLayer::new(0.05, polys)
            })
            .collect();
        let plate = PlateSlices {
            contours: ContourStack::new_unchecked(layers),
            slab_tops,
        };

        let payload = ProjectPayload {
            printer: Some(printer.clone()),
            resin: Some(resin.clone()),
            first_layer_height: None,
            embedded: Vec::new(),
        };

        let mut bytes = Vec::new();
        write_project(
            &scene,
            &payload,
            EmbedProfiles::No,
            Some(&plate),
            Some(&token),
            three_mf(),
            &mut bytes,
        )
        .expect("the project writes cleanly");
        let loaded = read_project(&mut Cursor::new(bytes), three_mf()).expect("the project reads");

        assert!(
            loaded.slices.is_some(),
            "the persisted slice geometry must read back"
        );
        assert_eq!(loaded.slice_fingerprint.as_deref(), Some(token.as_str()));
        assert!(
            revalidate_persisted_slices(
                loaded.payload.as_ref(),
                &loaded.scene,
                loaded.slices,
                loaded.slice_fingerprint,
            )
            .is_some(),
            "a reopened project's slices must revalidate against the reloaded scene"
        );
    }
}
