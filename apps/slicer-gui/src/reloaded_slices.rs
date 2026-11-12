/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Revalidation of a reopened project's persisted slices.
//!
//! A saved project carries its merged slice geometry plus a validity token
//! fingerprinting the scene geometry, the resolved layer schedule and the
//! resin's scale compensation. On load
//! the slices are trusted only when that token still matches: the profiles are
//! re-resolved from the installed library and the fingerprint recomputed
//! against the reopened scene, so a profile or geometry change since the save
//! discards the stale slices and the scene opens slice-less. Both steps (disk
//! profile resolution and the geometry hash) run on the load worker, off the
//! event loop.
//!
//! The revalidated slices ride to the UI thread and become the held
//! [`SliceResult`] at install time, when the scene-geometry marker is known.

use sc_print_glue::{PersistedSliceValidity, PlateContours, validate_persisted_slices};
use sc_print_primitives::{AaLevel, MaterialId, PositiveFiniteMm, PrinterId, ScaleCompensation};
use sc_print_profiles::resolve_scale_compensation;
use sc_project_formats::{PersistedSlices, ProjectPayload};
use sc_scene::Scene;
use sc_sl1cer::profile::LayerSchedule;

use crate::scene_document::SceneRevision;
use crate::slice_projection::{load_selected_profiles, schedule_for_profiles};
use crate::slice_result::{LayerValues, SliceParams, SliceResult};

/// A project's persisted slices that passed revalidation on load: the reloaded
/// contour stack paired with its schedule, plus the inputs they were validated
/// against. Built on the worker and turned into a held [`SliceResult`] at
/// install time.
pub(crate) struct ReloadedSlices {
    contours: PlateContours,
    token: String,
    printer: PrinterId,
    resin: MaterialId,
    first_layer_height: Option<PositiveFiniteMm>,
    layers: Option<LayerValues>,
}

impl ReloadedSlices {
    /// Builds the held slice result, stamping the scene-geometry marker
    /// captured after the fresh install, so the reloaded slice reads as in sync
    /// and re-persists without a re-slice. The anti-aliasing level is not
    /// persisted and does not feed the token, so it defaults to `Off`.
    pub(crate) fn into_slice_result(self, geometry: SceneRevision) -> SliceResult {
        SliceResult {
            contours: self.contours,
            token: self.token,
            params: SliceParams {
                printer: self.printer,
                resin: self.resin,
                first_layer_height: self.first_layer_height,
                aa_level: AaLevel::Off,
                geometry,
                plate: 0,
                plate_key: None,
            },
            layers: self.layers,
        }
    }
}

/// Revalidates a project's persisted slices against the reopened scene,
/// returning them only when they are still in sync. Resolves the layer schedule
/// and the scale compensation from the installed profiles the same way the
/// slice does, then recomputes the fingerprint and compares it to the stored
/// token. A payload without a printer or resin, a schedule that no longer
/// resolves, a stack whose layer count differs from the schedule's, or a token
/// mismatch (a compensation edited since the save, or a token saved before the
/// fingerprint covered it) yields `None`, and the scene loads slice-less.
pub(crate) fn revalidate_persisted_slices(
    payload: &ProjectPayload,
    scene: &Scene,
    persisted: PersistedSlices,
) -> Option<ReloadedSlices> {
    let PersistedSlices {
        contours: stack,
        fingerprint: token,
    } = persisted;
    let printer = payload.printer.clone()?;
    let resin = payload.resin.clone()?;
    let first_layer_height = payload.first_layer_height;

    let (printer_profile, resin_profile) = load_selected_profiles(&printer, &resin).ok()?;
    let schedule =
        schedule_for_profiles(&printer_profile, &resin_profile, first_layer_height, scene).ok()?;
    let contours = PlateContours::new(stack, schedule).ok()?;
    let scale = resolve_scale_compensation(&printer_profile, &resin_profile);
    if !slices_in_sync(&token, scene, contours.schedule(), scale) {
        return None;
    }
    Some(ReloadedSlices {
        contours,
        token,
        printer,
        resin,
        first_layer_height,
        // The token pins the schedule and the scale compensation, so the
        // slices were cut with what the profiles hold at load.
        layers: LayerValues::resolve(&printer_profile, &resin_profile, first_layer_height),
    })
}

/// Reports whether the stored token still matches the fingerprint of the scene
/// under the resolved schedule and scale compensation.
fn slices_in_sync(
    token: &str,
    scene: &Scene,
    schedule: &LayerSchedule,
    scale: ScaleCompensation,
) -> bool {
    matches!(
        validate_persisted_slices(token, scene, schedule, scale),
        PersistedSliceValidity::InSync
    )
}

#[cfg(test)]
mod reloaded_slices_tests {
    use super::*;

    use std::io::Cursor;
    use std::sync::Arc;

    use sc_mesh_core::transform::Transform3D;
    use sc_print_glue::slice_input_fingerprint;
    use sc_project_formats::{EmbedProfiles, read_project, write_project};
    use sc_scene::node::LeafPayload;
    use sc_sl1cer::contour::{ContourLayer, ContourStack};

    use crate::slice_projection::resolve_current_schedule;
    use sc_print_primitives::ScaleCompensationPercent;

    use crate::test_support::projects::{
        cube_mesh, project_with_in_sync_slices, resolved_scale, three_mf,
    };

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
        token_under(scene, schedule, ScaleCompensation::NONE)
    }

    fn token_under(scene: &Scene, schedule: &LayerSchedule, scale: ScaleCompensation) -> String {
        slice_input_fingerprint(scene, schedule, scale)
            .expect("the scene has sliceable geometry")
            .to_token()
    }

    fn empty_layers_at(zs: impl Iterator<Item = f64>) -> ContourStack {
        ContourStack::new(zs.map(|z| ContourLayer::new(z, vec![])).collect())
            .expect("z is strictly increasing")
    }

    fn empty_payload() -> ProjectPayload {
        ProjectPayload {
            printer: None,
            resin: None,
            first_layer_height: None,
            output_format: None,
            embedded: Vec::new(),
            review_decisions: Vec::new(),
            plates: 0,
            plate_names: Vec::new(),
            hand_placed: Vec::new(),
        }
    }

    #[test]
    fn the_fingerprint_survives_a_project_write_read_round_trip() {
        // The whole feature rests on this: if the scene fingerprint did not
        // survive a save/reopen, every reopened project would read as stale and
        // never show its slices.
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
            &mut Cursor::new(&mut bytes),
        )
        .expect("the project writes cleanly");

        let loaded = read_project(&mut Cursor::new(bytes), three_mf()).expect("the project reads");
        assert!(
            slices_in_sync(&token, &loaded.scene, &schedule, ScaleCompensation::NONE),
            "the fingerprint must match after a project round trip"
        );
        assert_eq!(loaded.slice_fingerprint.as_deref(), Some(token.as_str()));
    }

    #[test]
    fn a_matching_token_is_in_sync_and_a_wrong_one_is_not() {
        let scene = cube_scene();
        let schedule = schedule();
        let token = token_for(&scene, &schedule);

        assert!(slices_in_sync(
            &token,
            &scene,
            &schedule,
            ScaleCompensation::NONE
        ));
        assert!(!slices_in_sync(
            "not-the-real-token",
            &scene,
            &schedule,
            ScaleCompensation::NONE
        ));
    }

    #[test]
    fn a_changed_schedule_is_not_in_sync() {
        // The token covers the resolved schedule by value, so slicing at a
        // different layer height must invalidate it.
        let scene = cube_scene();
        let token = token_for(&scene, &schedule());
        let other = LayerSchedule::uniform(0.1, 0.05, 20).expect("a valid schedule");
        assert!(!slices_in_sync(
            &token,
            &scene,
            &other,
            ScaleCompensation::NONE
        ));
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
            contours: PlateContours::new(empty_layers_at(schedule.iter_z()), schedule)
                .expect("1 layer for each scheduled layer"),
            token: token.clone(),
            printer: printer.clone(),
            resin: resin.clone(),
            first_layer_height,
            layers: None,
        };
        let geometry = SceneRevision::from_top_edit_id(None);
        let result = reloaded.into_slice_result(geometry);

        assert_eq!(result.contours.schedule(), &schedule);
        assert_eq!(result.contours.stack().layers().len(), 40);
        assert_eq!(result.token, token);
        assert_eq!(result.params.printer, printer);
        assert_eq!(result.params.resin, resin);
        assert_eq!(result.params.first_layer_height, first_layer_height);
        assert_eq!(result.params.aa_level, AaLevel::Off);
        assert_eq!(result.params.geometry, geometry);
    }

    #[test]
    fn a_payload_without_profiles_yields_no_reloaded_slices() {
        let scene = cube_scene();
        let persisted = PersistedSlices {
            contours: ContourStack::new_unchecked(Vec::new()),
            fingerprint: token_for(&scene, &schedule()),
        };
        // A payload without a printer or resin cannot resolve a schedule.
        assert!(revalidate_persisted_slices(&empty_payload(), &scene, persisted).is_none());
    }

    #[test]
    fn a_saved_stack_of_another_layer_count_reopens_without_slices() {
        // The token covers only the scene, the schedule and the compensation,
        // so a matching token does not prove that the saved layer count is
        // correct.
        let scene = cube_scene();
        let printer = PrinterId::new("elegoo-mars-5").expect("valid id");
        let resin = MaterialId::new("siraya-tech-craft-high-clear").expect("valid id");
        let schedule = resolve_current_schedule(&printer, &resin, None, &scene)
            .expect("the bundled default profiles resolve a schedule");
        let token = token_under(&scene, &schedule, resolved_scale(&printer, &resin));
        let payload = ProjectPayload {
            printer: Some(printer),
            resin: Some(resin),
            ..empty_payload()
        };
        let reopen = |stack: ContourStack| {
            revalidate_persisted_slices(
                &payload,
                &scene,
                PersistedSlices {
                    contours: stack,
                    fingerprint: token.clone(),
                },
            )
        };

        assert!(reopen(empty_layers_at(schedule.iter_z())).is_some());
        assert!(reopen(empty_layers_at(schedule.iter_z().skip(1))).is_none());
    }

    #[test]
    fn slices_cut_under_another_scale_compensation_reopen_stale() {
        // A project saved, then reopened after only the resin's X/Y scale
        // compensation changed: the schedule still matches, the layers do not
        // (#295).
        let scene = cube_scene();
        let printer = PrinterId::new("elegoo-mars-5").expect("valid id");
        let resin = MaterialId::new("siraya-tech-craft-high-clear").expect("valid id");
        let schedule = resolve_current_schedule(&printer, &resin, None, &scene)
            .expect("the bundled default profiles resolve a schedule");
        let current = resolved_scale(&printer, &resin);
        let shifted = |percent: ScaleCompensationPercent| {
            ScaleCompensationPercent::new(if percent.value() > 0.0 { 0.0 } else { 0.5 })
                .expect("within +-10 %")
        };
        let other = ScaleCompensation {
            x: shifted(current.x),
            y: shifted(current.y),
            z: current.z,
        };
        let payload = ProjectPayload {
            printer: Some(printer),
            resin: Some(resin),
            ..empty_payload()
        };
        let reopen = |token: String| {
            revalidate_persisted_slices(
                &payload,
                &scene,
                PersistedSlices {
                    contours: empty_layers_at(schedule.iter_z()),
                    fingerprint: token,
                },
            )
        };

        assert!(reopen(token_under(&scene, &schedule, current)).is_some());
        assert!(reopen(token_under(&scene, &schedule, other)).is_none());
    }

    #[test]
    fn a_reopened_project_revalidates_slices_saved_on_a_placed_scene() {
        // Reproduces the reopen scenario: a project saved with real slice
        // geometry on the bundled default profiles, then reopened. The full
        // reopen path (read back the standalone slice part, re-resolve the
        // schedule, recompute the fingerprint) must accept the slices. The
        // saved and the reopened scene nest the same geometry differently, so
        // the fingerprint must ignore the nesting, or the reopened slices are
        // wrongly rejected as stale.
        let (bytes, token) = project_with_in_sync_slices();
        let loaded = read_project(&mut Cursor::new(bytes), three_mf()).expect("the project reads");

        assert!(
            loaded.slices.is_some(),
            "the persisted slice geometry must read back"
        );
        assert_eq!(loaded.slice_fingerprint.as_deref(), Some(token.as_str()));
        assert!(
            revalidate_persisted_slices(
                loaded
                    .payload
                    .as_ref()
                    .expect("the project carries its payload"),
                &loaded.scene,
                PersistedSlices {
                    contours: loaded.slices.expect("checked above"),
                    fingerprint: loaded.slice_fingerprint.expect("checked above"),
                },
            )
            .is_some(),
            "a reopened project's slices must revalidate against the reloaded scene"
        );
    }
}
