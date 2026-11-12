/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Dirty-tracking behavior of [`SceneDocument`]: the save-point comparison, the
//! eviction rule, and the exactness of the `DirtyChanged` event.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::NodeId;

use crate::scene_document::{
    SaveRecorded, SceneDocument, SceneEdit, SceneEvent, SceneSubscription, UNDO_STACK_CAP,
};
use crate::test_support::documents::loaded_document;
use crate::test_support::scenes::graph_with_one_object;

fn fixture_path() -> PathBuf {
    PathBuf::from("/tmp/dirty_fixture.obj")
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

fn capture_events(doc: &Rc<SceneDocument>) -> (Rc<RefCell<Vec<SceneEvent>>>, SceneSubscription) {
    let log: Rc<RefCell<Vec<SceneEvent>>> = Rc::default();
    let sink = log.clone();
    let sub = doc.subscribe(move |event| sink.borrow_mut().push(event));
    (log, sub)
}

mod baseline_tests {
    use super::*;

    #[test]
    fn a_fresh_document_is_clean() {
        assert!(!SceneDocument::new().is_dirty());
    }

    #[test]
    fn a_freshly_installed_scene_is_clean() {
        let (doc, _) = loaded_document();
        assert!(!doc.is_dirty());
    }

    #[test]
    fn a_committed_edit_marks_the_document_dirty() {
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        assert!(doc.is_dirty());
    }

    #[test]
    fn a_mid_gesture_mutation_without_a_commit_stays_clean() {
        let (doc, node_id) = loaded_document();
        assert!(
            doc.apply_edit(SceneEdit::SetLocalTransform {
                node_id,
                before: Transform3D::Identity,
                after: translation(1.0),
            })
            .is_ok()
        );
        assert!(!doc.is_dirty());
    }

    #[test]
    fn a_no_op_commit_stays_clean() {
        let (doc, node_id) = loaded_document();
        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: Transform3D::Identity,
        });
        assert!(!doc.is_dirty());
    }

    #[test]
    fn mark_saved_returns_the_document_to_clean() {
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        doc.mark_saved();
        assert!(!doc.is_dirty());
    }

    #[test]
    fn mark_saved_with_an_empty_stack_keeps_the_document_clean() {
        let (doc, _) = loaded_document();
        doc.mark_saved();
        assert!(!doc.is_dirty());
    }
}

mod save_point_tests {
    use super::*;

    #[test]
    fn edits_after_a_save_read_dirty_and_undo_back_to_the_save_point_reads_clean() {
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        commit_step(&doc, node_id, 2.0);
        doc.mark_saved();
        commit_step(&doc, node_id, 3.0);
        assert!(doc.is_dirty());

        assert!(doc.undo().unwrap());
        assert!(!doc.is_dirty());
    }

    #[test]
    fn undoing_below_the_save_point_reads_dirty() {
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        commit_step(&doc, node_id, 2.0);
        doc.mark_saved();

        assert!(doc.undo().unwrap());
        assert!(doc.is_dirty());
    }

    #[test]
    fn redoing_back_to_the_save_point_reads_clean_again() {
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        commit_step(&doc, node_id, 2.0);
        doc.mark_saved();
        doc.undo().unwrap();

        assert!(doc.redo().unwrap());
        assert!(!doc.is_dirty());
    }

    #[test]
    fn a_new_edit_branch_after_undoing_from_the_save_point_reads_dirty() {
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        commit_step(&doc, node_id, 2.0);
        doc.mark_saved();
        doc.undo().unwrap();

        // The new branch replaces the saved top entry; its fresh id can never
        // match the save point again.
        commit_step(&doc, node_id, 5.0);
        assert!(doc.is_dirty());
        doc.undo().unwrap();
        assert!(doc.is_dirty());
    }
}

mod eviction_tests {
    use super::*;

    #[test]
    fn undoing_every_edit_without_eviction_reads_clean() {
        // open -> change -> undo everything: nothing was ever evicted, so the
        // drained stack proves the content is back at the freshly-installed
        // state.
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        commit_step(&doc, node_id, 2.0);
        assert!(doc.undo().unwrap());
        assert!(doc.is_dirty());
        assert!(doc.undo().unwrap());
        assert!(!doc.is_dirty());
    }

    #[test]
    fn redo_back_to_a_saved_top_after_draining_reads_clean() {
        // Draining the stack past a saved top entry leaves no lasting mark: the
        // entry's identity proves the content once redo puts it back on top.
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        doc.mark_saved();
        assert!(doc.undo().unwrap());
        assert!(doc.is_dirty());
        assert!(doc.redo().unwrap());
        assert!(!doc.is_dirty());
    }

    #[test]
    fn eviction_plus_full_undo_never_reads_clean() {
        // The document was last saved with an EMPTY stack (the
        // freshly-installed state). Committing 1 entry more than the cap evicts
        // the oldest edit; undoing everything then drains the stack without
        // reaching the installed content. Top (None) equals the save point
        // (None) here, so only the eviction rule stands between this state and
        // a false clean.
        let (doc, node_id) = loaded_document();
        for i in 0..(UNDO_STACK_CAP + 1) {
            commit_step(&doc, node_id, (i + 1) as f64);
        }
        for _ in 0..UNDO_STACK_CAP {
            assert!(doc.undo().unwrap());
        }
        assert!(!doc.can_undo());
        assert!(doc.is_dirty());
    }

    #[test]
    fn a_save_resets_the_eviction_rule() {
        // After an eviction the empty-vs-empty comparison is untrusted, but
        // only until the next save records a new reference state.
        let (doc, node_id) = loaded_document();
        for i in 0..(UNDO_STACK_CAP + 1) {
            commit_step(&doc, node_id, (i + 1) as f64);
        }
        for _ in 0..UNDO_STACK_CAP {
            assert!(doc.undo().unwrap());
        }
        assert!(doc.is_dirty());

        doc.mark_saved();
        assert!(!doc.is_dirty());

        // A fresh change-then-undo cycle on top of the new save point reads
        // clean again. The undo of edit 1 was evicted, so the content sits at
        // translation(1.0); the next step in the chain is 2.0.
        commit_step(&doc, node_id, 2.0);
        assert!(doc.is_dirty());
        assert!(doc.undo().unwrap());
        assert!(!doc.is_dirty());
    }
}

mod reset_tests {
    use super::*;

    #[test]
    fn installing_a_new_scene_resets_dirty() {
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        assert!(doc.is_dirty());
        doc.install_graph(graph_with_one_object(), fixture_path());
        assert!(!doc.is_dirty());
    }

    #[test]
    fn clearing_the_scene_resets_dirty() {
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        doc.clear_scene();
        assert!(!doc.is_dirty());
    }

    #[test]
    fn the_eviction_rule_does_not_survive_an_install() {
        let (doc, node_id) = loaded_document();
        for i in 0..(UNDO_STACK_CAP + 1) {
            commit_step(&doc, node_id, (i + 1) as f64);
        }

        doc.install_graph(graph_with_one_object(), fixture_path());
        let node_id = doc
            .with_scene(|s| s.leaf_id_at_index(0))
            .flatten()
            .expect("fixture has one leaf");

        // Were the eviction flag still set, this change-then-undo cycle would
        // read dirty at the end.
        commit_step(&doc, node_id, 1.0);
        assert!(doc.undo().unwrap());
        assert!(!doc.is_dirty());
    }
}

mod dirty_changed_event_tests {
    use super::*;

    #[test]
    fn the_event_fires_only_on_actual_flips() {
        let (doc, node_id) = loaded_document();
        let (log, _sub) = capture_events(&doc);

        let dirty_flips = |log: &Rc<RefCell<Vec<SceneEvent>>>| {
            log.borrow()
                .iter()
                .filter(|e| **e == SceneEvent::DirtyChanged)
                .count()
        };

        commit_step(&doc, node_id, 1.0); // clean -> dirty
        assert_eq!(dirty_flips(&log), 1);

        commit_step(&doc, node_id, 2.0); // dirty -> dirty
        assert_eq!(dirty_flips(&log), 1);

        doc.mark_saved(); // dirty -> clean
        assert_eq!(dirty_flips(&log), 2);

        doc.mark_saved(); // clean -> clean
        assert_eq!(dirty_flips(&log), 2);

        doc.undo().unwrap(); // clean -> dirty (below the save point)
        assert_eq!(dirty_flips(&log), 3);

        doc.redo().unwrap(); // dirty -> clean (back at the save point)
        assert_eq!(dirty_flips(&log), 4);
    }

    #[test]
    fn the_event_fires_after_the_stack_event() {
        let (doc, node_id) = loaded_document();
        let (log, _sub) = capture_events(&doc);

        doc.push_edit(SceneEdit::SetLocalTransform {
            node_id,
            before: Transform3D::Identity,
            after: translation(1.0),
        });
        assert_eq!(
            *log.borrow(),
            vec![SceneEvent::UndoStackChanged, SceneEvent::DirtyChanged]
        );
    }

    #[test]
    fn clearing_a_dirty_document_reports_the_flip() {
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        let (log, _sub) = capture_events(&doc);

        doc.clear_scene();
        assert!(log.borrow().contains(&SceneEvent::DirtyChanged));
    }
}

/// The scene's own print choices are project-level state that the payload
/// holds, apart from the undo stack. A change marks the document save-dirty and
/// leaves the undo history as it is; a save clears the mark.
mod unsaved_project_state_tests {
    use super::*;
    use sc_print_primitives::{
        FormatKind, MaterialId, NonEmptyString, OutputFormatBinding, PositiveFiniteMm, PrinterId,
    };
    use sc_project_formats::ProjectPayload;

    use crate::print_selection::PrintDefaults;

    fn printer(id: &str) -> PrinterId {
        PrinterId::new(id).expect("valid printer id")
    }

    fn resin(id: &str) -> MaterialId {
        MaterialId::new(id).expect("valid material id")
    }

    fn mm(value: f64) -> PositiveFiniteMm {
        PositiveFiniteMm::new(value).expect("valid length")
    }

    #[test]
    fn choosing_a_printer_marks_the_document_dirty() {
        let (doc, _) = loaded_document();
        assert!(doc.set_selected_printer(Some(printer("anycubic-photon"))));
        assert!(doc.is_dirty());
        assert_eq!(
            doc.print_selection().printer,
            Some(printer("anycubic-photon"))
        );
    }

    #[test]
    fn choosing_a_resin_marks_the_document_dirty() {
        let (doc, _) = loaded_document();
        assert!(doc.set_selected_resin(Some(resin("generic-grey"))));
        assert!(doc.is_dirty());
        assert_eq!(doc.print_selection().resin, Some(resin("generic-grey")));
    }

    #[test]
    fn setting_a_first_layer_height_marks_the_document_dirty() {
        let (doc, _) = loaded_document();
        assert!(doc.set_selected_first_layer_height(Some(mm(0.05))));
        assert!(doc.is_dirty());
        assert_eq!(doc.print_selection().first_layer_height, Some(mm(0.05)));
    }

    #[test]
    fn re_setting_the_same_first_layer_height_is_a_no_op() {
        let (doc, _) = loaded_document();
        doc.set_selected_first_layer_height(Some(mm(0.05)));
        doc.mark_saved();
        assert!(!doc.is_dirty());

        let (log, _sub) = capture_events(&doc);
        assert!(!doc.set_selected_first_layer_height(Some(mm(0.05))));
        assert!(!doc.is_dirty());
        assert!(!log.borrow().contains(&SceneEvent::DirtyChanged));
    }

    #[test]
    fn clearing_a_first_layer_height_that_was_never_set_is_a_no_op() {
        let (doc, _) = loaded_document();
        assert!(!doc.set_selected_first_layer_height(None));
        assert!(!doc.is_dirty());
        assert_eq!(doc.print_selection().first_layer_height, None);
    }

    #[test]
    fn re_choosing_the_same_printer_is_a_no_op() {
        let (doc, _) = loaded_document();
        doc.set_selected_printer(Some(printer("anycubic-photon")));
        doc.mark_saved();
        assert!(!doc.is_dirty());

        // Same value: no change, no dirty, and no reported flip.
        let (log, _sub) = capture_events(&doc);
        assert!(!doc.set_selected_printer(Some(printer("anycubic-photon"))));
        assert!(!doc.is_dirty());
        assert!(!log.borrow().contains(&SceneEvent::DirtyChanged));
    }

    #[test]
    fn clearing_a_selection_that_was_never_set_is_a_no_op() {
        let (doc, _) = loaded_document();
        assert!(!doc.set_selected_printer(None));
        assert!(!doc.is_dirty());
        assert_eq!(doc.print_selection().printer, None);
    }

    #[test]
    fn a_save_clears_the_unsaved_selection() {
        let (doc, _) = loaded_document();
        doc.set_selected_printer(Some(printer("anycubic-photon")));
        assert!(doc.is_dirty());
        doc.mark_saved();
        assert!(!doc.is_dirty());
    }

    #[test]
    fn installing_a_new_scene_clears_the_unsaved_selection() {
        let (doc, _) = loaded_document();
        doc.set_selected_printer(Some(printer("anycubic-photon")));
        assert!(doc.is_dirty());
        doc.install_graph(graph_with_one_object(), fixture_path());
        assert!(!doc.is_dirty());
    }

    #[test]
    fn an_unsaved_selection_keeps_the_document_dirty_after_undoing_edits() {
        // The save-dirty signal is the OR of undo-history changes and unsaved
        // project state: a pending selection change keeps the document dirty
        // after every geometry edit is undone back to the save point.
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        doc.mark_saved();
        doc.set_selected_printer(Some(printer("anycubic-photon")));
        commit_step(&doc, node_id, 2.0);
        assert!(doc.is_dirty());

        assert!(doc.undo().unwrap()); // back to the geometry save point
        assert!(doc.is_dirty()); // still dirty: the selection is unsaved
    }

    #[test]
    fn choosing_a_selection_records_it_without_making_the_document_a_project() {
        let (doc, _) = loaded_document();
        assert!(!doc.is_project());
        doc.set_selected_printer(Some(printer("anycubic-photon")));

        // The document's kind follows its file (a plain mesh here), whatever
        // print choices it holds.
        assert!(!doc.is_project());
        let own = doc.print_selection();
        assert_eq!(own.printer, Some(printer("anycubic-photon")));
        assert_eq!(own.resin, None);
        assert_eq!(
            doc.compose_project_payload(&PrintDefaults::default()),
            ProjectPayload {
                printer: Some(printer("anycubic-photon")),
                resin: None,
                first_layer_height: None,
                output_format: None,
                embedded: Vec::new(),
                review_decisions: Vec::new(),
                plates: 0,
                plate_names: Vec::new(),
                hand_placed: Vec::new(),
            }
        );
    }

    #[test]
    fn a_document_opened_from_a_project_stays_a_project_across_picks() {
        let doc = SceneDocument::new();
        doc.install_project_graph(
            graph_with_one_object(),
            PathBuf::from("/tmp/plate.3mf"),
            ProjectPayload::default(),
        );
        assert!(doc.is_project());
        assert!(doc.set_selected_printer(Some(printer("anycubic-photon"))));
        assert!(doc.is_project());
    }

    #[test]
    fn a_loaded_payload_becomes_the_selection_and_the_held_embedded_documents() {
        use sc_project_formats::EmbeddedProfile;

        let loaded = ProjectPayload {
            printer: Some(printer("anycubic-photon")),
            resin: None,
            first_layer_height: Some(mm(0.1)),
            output_format: None,
            embedded: vec![EmbeddedProfile::Printer {
                id: printer("anycubic-photon"),
                bytes: b"kind = \"printer\"".to_vec(),
            }],
            review_decisions: Vec::new(),
            plates: 0,
            plate_names: Vec::new(),
            hand_placed: Vec::new(),
        };
        let doc = SceneDocument::new();
        doc.install_project_graph(
            graph_with_one_object(),
            PathBuf::from("/tmp/plate.3mf"),
            loaded.clone(),
        );

        let own = doc.print_selection();
        assert_eq!(own.printer, Some(printer("anycubic-photon")));
        assert_eq!(own.resin, None);
        assert_eq!(own.first_layer_height, Some(mm(0.1)));
        // Composing with defaults fills only the missing field.
        let composed = doc.compose_project_payload(&PrintDefaults {
            printer: Some(printer("global-printer")),
            resin: Some(resin("global-resin")),
        });
        assert_eq!(composed.printer, Some(printer("anycubic-photon")));
        assert_eq!(composed.resin, Some(resin("global-resin")));
        assert_eq!(composed.first_layer_height, Some(mm(0.1)));
        assert_eq!(composed.embedded, loaded.embedded);

        // Clearing the scene drops the choices and the embedded documents.
        doc.clear_scene();
        assert_eq!(doc.print_selection().printer, None);
        assert_eq!(doc.print_selection().first_layer_height, None);
    }

    #[test]
    fn a_loaded_output_format_survives_back_into_the_composed_payload() {
        let loaded = ProjectPayload {
            output_format: Some(OutputFormatBinding {
                format: FormatKind::Goo,
                version: Some(NonEmptyString::new("v3.0").expect("non-empty version")),
            }),
            ..ProjectPayload::default()
        };
        let doc = SceneDocument::new();
        doc.install_project_graph(
            graph_with_one_object(),
            PathBuf::from("/tmp/plate.3mf"),
            loaded.clone(),
        );

        assert_eq!(
            doc.compose_project_payload(&PrintDefaults::default())
                .output_format,
            loaded.output_format,
        );

        doc.clear_scene();
        assert_eq!(
            doc.compose_project_payload(&PrintDefaults::default())
                .output_format,
            None
        );
    }

    #[test]
    fn printer_and_resin_choices_coexist_on_the_selection() {
        let (doc, _) = loaded_document();
        doc.set_selected_printer(Some(printer("anycubic-photon")));
        doc.set_selected_resin(Some(resin("generic-grey")));

        let own = doc.print_selection();
        assert_eq!(own.printer, Some(printer("anycubic-photon")));
        assert_eq!(own.resin, Some(resin("generic-grey")));
    }

    #[test]
    fn the_dirty_event_fires_once_across_a_run_of_selection_changes() {
        let (doc, _) = loaded_document();
        let (log, _sub) = capture_events(&doc);
        let flips = |log: &Rc<RefCell<Vec<SceneEvent>>>| {
            log.borrow()
                .iter()
                .filter(|e| **e == SceneEvent::DirtyChanged)
                .count()
        };

        doc.set_selected_printer(Some(printer("anycubic-photon"))); // clean -> dirty
        assert_eq!(flips(&log), 1);
        doc.set_selected_resin(Some(resin("generic-grey"))); // dirty -> dirty
        assert_eq!(flips(&log), 1);
        doc.mark_saved(); // dirty -> clean
        assert_eq!(flips(&log), 2);
    }
}

/// The save flow snapshots the payload on the UI thread, writes on a worker
/// while the event loop stays live, then records the result. These tests act in
/// that gap: whatever lands between the snapshot and the recording is NOT in
/// the written file.
#[cfg(test)]
mod save_completion_guard_tests {
    use super::*;

    use sc_print_primitives::PrinterId;

    use crate::print_selection::PrintDefaults;

    fn target() -> PathBuf {
        PathBuf::from("/tmp/dirty_fixture.scproj")
    }

    #[test]
    fn an_edit_landing_during_the_write_keeps_the_document_dirty() {
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        let payload = doc.compose_project_payload(&PrintDefaults::default());
        let stamp = doc.save_stamp();

        // The write is in flight from here.
        commit_step(&doc, node_id, 2.0);
        assert_eq!(
            doc.record_project_save_for(payload, target(), stamp),
            SaveRecorded::Drifted,
        );

        assert!(
            doc.is_dirty(),
            "the second edit is not in the written file, so it is unsaved",
        );
    }

    #[test]
    fn a_printer_change_during_the_write_survives_the_recording() {
        let (doc, _) = loaded_document();
        let payload = doc.compose_project_payload(&PrintDefaults::default());
        let stamp = doc.save_stamp();

        let chosen = PrinterId::new("elegoo-mars-4").expect("valid id");
        assert!(doc.set_selected_printer(Some(chosen.clone())));
        assert_eq!(
            doc.record_project_save_for(payload, target(), stamp),
            SaveRecorded::Drifted,
        );

        assert_eq!(
            doc.print_selection().printer,
            Some(chosen),
            "the recording must not overwrite a choice made after the snapshot",
        );
        assert!(doc.is_dirty(), "that choice is not in the written file");
    }

    #[test]
    fn an_undisturbed_write_records_the_project_and_cleans_the_document() {
        let (doc, node_id) = loaded_document();
        commit_step(&doc, node_id, 1.0);
        let payload = doc.compose_project_payload(&PrintDefaults::default());
        let stamp = doc.save_stamp();

        assert_eq!(
            doc.record_project_save_for(payload, target(), stamp),
            SaveRecorded::Clean,
        );
        assert!(!doc.is_dirty());
        assert!(doc.is_project());
        assert_eq!(doc.source_path(), Some(target()));
    }

    #[test]
    fn a_drifted_write_still_becomes_the_save_target() {
        let (doc, node_id) = loaded_document();
        let payload = doc.compose_project_payload(&PrintDefaults::default());
        let stamp = doc.save_stamp();

        commit_step(&doc, node_id, 1.0);
        assert_eq!(
            doc.record_project_save_for(payload, target(), stamp),
            SaveRecorded::Drifted,
        );
        assert_eq!(
            doc.source_path(),
            Some(target()),
            "a later Save overwrites the file this write produced",
        );
        assert!(doc.is_project());
    }

    #[test]
    fn a_scene_replaced_during_the_write_is_not_relabeled() {
        let (doc, _) = loaded_document();
        let payload = doc.compose_project_payload(&PrintDefaults::default());
        let stamp = doc.save_stamp();

        let replacement = PathBuf::from("/tmp/other_fixture.obj");
        doc.install_graph(graph_with_one_object(), replacement.clone());
        assert_eq!(
            doc.record_project_save_for(payload, target(), stamp),
            SaveRecorded::Replaced,
        );

        assert_eq!(
            doc.source_path(),
            Some(replacement),
            "the replacement scene keeps its own path",
        );
        assert!(
            !doc.is_project(),
            "a plain mesh must not become the saved project",
        );
    }
}
