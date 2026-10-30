/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! [`SceneDocument`] side of the slice result: holding the produced slice,
//! capturing a scene revision, and the geometry-change comparison the re-slice
//! decision relies on (including the empty-stack eviction guard, mirrored from
//! the save-point tracking).

use std::path::PathBuf;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_print_glue::PlateContours;
use sc_print_primitives::{AaLevel, MaterialId, PrinterId};
use sc_scene::NodeId;
use sc_sl1cer::contour::{ContourLayer, ContourStack};
use sc_sl1cer::profile::LayerSchedule;

use crate::scene_document::{SceneDocument, SceneEdit, SceneRevision, UNDO_STACK_CAP};
use crate::slice_result::{SliceParams, SliceResult};
use crate::test_support::documents::loaded_document;
use crate::test_support::scenes::graph_with_one_object;

fn fixture_path() -> PathBuf {
    PathBuf::from("/tmp/slice_result_fixture.obj")
}

fn translation(dx: f64) -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_translation([dx, 0.0, 0.0]))
}

/// Applies and commits the `dx - 1.0 -> dx` translation step, the well-formed
/// chain shape undo expects.
fn commit_step(doc: &SceneDocument, node_id: NodeId, dx: f64) {
    let edit = SceneEdit::SetLocalTransform {
        node_id,
        before: if dx == 1.0 {
            Transform3D::Identity
        } else {
            translation(dx - 1.0)
        },
        after: translation(dx),
    };
    assert!(doc.apply_edit(edit.clone()).is_ok());
    doc.push_edit(edit);
}

fn sample_schedule() -> LayerSchedule {
    LayerSchedule::uniform(0.05, 0.05, 1).expect("a positive, non-empty schedule")
}

fn contours_on(schedule: LayerSchedule) -> PlateContours {
    let layers = schedule
        .iter_z()
        .map(|z| ContourLayer::new(z, vec![]))
        .collect();
    let stack = ContourStack::new(layers).expect("z is strictly increasing");
    PlateContours::new(stack, schedule).expect("1 layer for each scheduled layer")
}

fn sample_result(geometry: SceneRevision) -> SliceResult {
    SliceResult {
        contours: contours_on(sample_schedule()),
        token: "sample-token".to_string(),
        params: SliceParams {
            printer: PrinterId::new("elegoo-saturn-4-ultra").expect("valid id"),
            resin: MaterialId::new("standard-405").expect("valid id"),
            first_layer_height: None,
            aa_level: AaLevel::Off,
            geometry,
        },
        layers: None,
    }
}

#[test]
fn a_fresh_document_holds_no_slice_result() {
    let (doc, _) = loaded_document();
    assert!(doc.slice_params().is_none());
}

#[test]
fn set_slice_result_stores_the_params() {
    let (doc, _) = loaded_document();
    let rev = doc.scene_revision();
    doc.set_slice_result(sample_result(rev));
    let held = doc.slice_params().expect("a result was stored");
    assert_eq!(held.geometry, rev);
    assert_eq!(held.aa_level, AaLevel::Off);
}

#[test]
fn a_committed_edit_moves_the_scene_revision() {
    let (doc, node_id) = loaded_document();
    let before = doc.scene_revision();
    commit_step(&doc, node_id, 1.0);
    assert_ne!(doc.scene_revision(), before);
}

#[test]
fn replacing_the_scene_reads_changed_even_at_an_empty_undo_stack() {
    // Both scenes sit at an empty stack, so only the scene half of the revision
    // separates them. A marker taken against the outgoing scene describes
    // geometry the document no longer holds.
    let (doc, _) = loaded_document();
    let rev = doc.scene_revision();
    doc.install_graph(graph_with_one_object(), fixture_path());
    assert_ne!(doc.scene_revision(), rev);
    assert!(doc.slice_geometry_changed(rev));
}

#[test]
fn clearing_the_scene_reads_changed_even_at_an_empty_undo_stack() {
    let (doc, _) = loaded_document();
    let rev = doc.scene_revision();
    doc.clear_scene();
    assert!(doc.slice_geometry_changed(rev));
}

#[test]
fn an_unchanged_scene_reads_unchanged_against_the_slice_point() {
    let (doc, node_id) = loaded_document();
    commit_step(&doc, node_id, 1.0);
    let rev = doc.scene_revision();
    doc.set_slice_result(sample_result(rev));
    assert!(!doc.slice_geometry_changed(rev));
}

#[test]
fn a_committed_edit_reads_changed_against_the_slice_point() {
    let (doc, node_id) = loaded_document();
    commit_step(&doc, node_id, 1.0);
    let rev = doc.scene_revision();
    doc.set_slice_result(sample_result(rev));
    commit_step(&doc, node_id, 2.0);
    assert!(doc.slice_geometry_changed(rev));
}

#[test]
fn undo_back_to_the_slice_point_reads_unchanged_again() {
    // A change followed by an undo back to the sliced state restores the exact
    // top edit id, so the comparison is precise here (no over-report), matching
    // the save-point tracking.
    let (doc, node_id) = loaded_document();
    commit_step(&doc, node_id, 1.0);
    let rev = doc.scene_revision();
    doc.set_slice_result(sample_result(rev));
    commit_step(&doc, node_id, 2.0);
    assert!(doc.slice_geometry_changed(rev));
    assert!(doc.undo().unwrap());
    assert!(!doc.slice_geometry_changed(rev));
    assert!(doc.redo().unwrap());
    assert!(doc.slice_geometry_changed(rev));
}

#[test]
fn an_empty_stack_slice_point_without_eviction_reads_unchanged_after_full_undo() {
    // Sliced with an empty undo stack, then a change-and-undo cycle that evicts
    // nothing: the drained stack proves the geometry is back at the sliced
    // state.
    let (doc, node_id) = loaded_document();
    let rev = doc.scene_revision();
    doc.set_slice_result(sample_result(rev));
    commit_step(&doc, node_id, 1.0);
    assert!(doc.slice_geometry_changed(rev));
    assert!(doc.undo().unwrap());
    assert!(!doc.slice_geometry_changed(rev));
}

#[test]
fn eviction_after_an_empty_stack_slice_point_reads_changed() {
    // Sliced with an empty undo stack (revision None). Committing one entry
    // past the cap evicts the oldest applied edit; undoing everything then
    // drains the stack back to empty without restoring the sliced geometry. Top
    // (None) equals the slice point (None), so only the eviction guard prevents
    // a false "unchanged".
    let (doc, node_id) = loaded_document();
    let rev = doc.scene_revision();
    doc.set_slice_result(sample_result(rev));
    for i in 0..(UNDO_STACK_CAP + 1) {
        commit_step(&doc, node_id, (i + 1) as f64);
    }
    for _ in 0..UNDO_STACK_CAP {
        assert!(doc.undo().unwrap());
    }
    assert!(!doc.can_undo());
    assert!(doc.slice_geometry_changed(rev));
}

#[test]
fn installing_a_new_scene_drops_the_held_slice_result() {
    let (doc, _) = loaded_document();
    doc.set_slice_result(sample_result(doc.scene_revision()));
    doc.install_graph(graph_with_one_object(), fixture_path());
    assert!(doc.slice_params().is_none());
}

#[test]
fn clearing_the_scene_drops_the_held_slice_result() {
    let (doc, _) = loaded_document();
    doc.set_slice_result(sample_result(doc.scene_revision()));
    doc.clear_scene();
    assert!(doc.slice_params().is_none());
}

#[test]
fn slice_layer_count_is_none_without_a_held_result() {
    let (doc, _) = loaded_document();
    assert!(doc.slice_layer_count().is_none());
}

#[test]
fn slice_layer_count_reports_the_held_schedule_layer_count() {
    let (doc, _) = loaded_document();
    let rev = doc.scene_revision();
    let result = SliceResult {
        contours: contours_on(
            LayerSchedule::uniform(0.05, 0.05, 3).expect("a positive, non-empty schedule"),
        ),
        ..sample_result(rev)
    };
    doc.set_slice_result(result);
    assert_eq!(doc.slice_layer_count(), Some(3));
}

#[test]
fn slice_layer_count_drops_to_none_after_clearing_the_scene() {
    let (doc, _) = loaded_document();
    doc.set_slice_result(sample_result(doc.scene_revision()));
    doc.clear_scene();
    assert!(doc.slice_layer_count().is_none());
}

#[test]
fn slice_layer_top_z_is_none_without_a_held_result() {
    let (doc, _) = loaded_document();
    assert!(doc.slice_layer_top_z(0).is_none());
}

#[test]
fn slice_layer_top_z_returns_the_layer_top() {
    let (doc, _) = loaded_document();
    let rev = doc.scene_revision();
    let schedule = LayerSchedule::uniform(0.05, 0.05, 3).expect("a positive, non-empty schedule");
    let expected: Vec<f64> = schedule.iter_slab_tops().collect();
    doc.set_slice_result(SliceResult {
        contours: contours_on(schedule),
        ..sample_result(rev)
    });
    assert_eq!(doc.slice_layer_top_z(0), Some(expected[0]));
    assert_eq!(doc.slice_layer_top_z(2), Some(expected[2]));
}

#[test]
fn slice_layer_top_z_is_none_past_the_last_layer() {
    let (doc, _) = loaded_document();
    let rev = doc.scene_revision();
    let schedule = LayerSchedule::uniform(0.05, 0.05, 3).expect("a positive, non-empty schedule");
    doc.set_slice_result(SliceResult {
        contours: contours_on(schedule),
        ..sample_result(rev)
    });
    assert!(doc.slice_layer_top_z(3).is_none());
}

#[test]
fn slices_for_persist_offers_the_slices_when_in_sync() {
    let (doc, _) = loaded_document();
    let rev = doc.scene_revision();
    doc.set_slice_result(sample_result(rev));
    let (plate, token) = doc
        .slices_for_persist(&sample_schedule())
        .expect("in-sync slices are offered for persistence");
    assert_eq!(token, "sample-token");
    assert_eq!(
        plate.slab_tops,
        sample_schedule().iter_slab_tops().collect::<Vec<_>>(),
    );
}

#[test]
fn slices_for_persist_is_none_without_a_held_result() {
    let (doc, _) = loaded_document();
    assert!(doc.slices_for_persist(&sample_schedule()).is_none());
}

#[test]
fn slices_for_persist_is_none_after_a_scene_edit() {
    let (doc, node_id) = loaded_document();
    let rev = doc.scene_revision();
    doc.set_slice_result(sample_result(rev));
    commit_step(&doc, node_id, 1.0);
    assert!(doc.slices_for_persist(&sample_schedule()).is_none());
}

#[test]
fn slices_for_persist_is_none_when_the_schedule_differs_by_value() {
    let (doc, _) = loaded_document();
    let rev = doc.scene_revision();
    doc.set_slice_result(sample_result(rev));
    // Same shape, a different layer height: a by-value schedule change, under
    // whatever profile identity, must withhold the slices.
    let thicker = LayerSchedule::uniform(0.10, 0.05, 1).expect("a positive, non-empty schedule");
    assert!(doc.slices_for_persist(&thicker).is_none());
}
