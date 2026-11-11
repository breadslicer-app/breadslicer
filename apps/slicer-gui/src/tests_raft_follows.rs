/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! A new raft in the Supports tab redraws the sets already made (#519): the
//! supports stay where they stand, only the raft under them changes, in one
//! undo step.

use super::*;

/// Every tip of `session`, rounded to a micron and sorted.
fn tips(session: &DfSupportSession) -> Vec<[i64; 3]> {
    let mut places: Vec<[i64; 3]> = session
        .tips()
        .into_iter()
        .map(|(_, p)| p.map(|c| (c * 1000.0).round() as i64))
        .collect();
    places.sort_unstable();
    places
}

#[test]
fn a_new_raft_redraws_the_supports_where_they_stand_in_one_undo_step() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run) = supported_slab(&window);
        let before = in_force_session(&run).expect("editable");
        let raft_before = before.options().expect("its options").raft;
        assert_ne!(raft_before, RaftMode::Lines);
        let (leaf, mesh_before) = run.support_leaf().expect("the supports");
        let bottom = |mesh: &sc_mesh_core::IndexedMesh| {
            mesh.vertices()
                .iter()
                .filter(|v| v.as_array()[2] < 0.05)
                .count()
        };

        // Lines: the same set, the same tips, drawn on another raft.
        run.on_option(9, 1.0);
        let after = in_force_session(&run).expect("still in force");
        assert_eq!(after.options().expect("its options").raft, RaftMode::Lines);
        assert_eq!(tips(&after), tips(&before), "the supports did not move");
        assert_eq!(after.hand_edited(), before.hand_edited());
        let (leaf_after, mesh_after) = run.support_leaf().expect("the supports");
        assert_eq!(leaf_after, leaf);
        assert!(!Arc::ptr_eq(&mesh_after, &mesh_before), "drawn again");
        assert_ne!(bottom(&mesh_after), bottom(&mesh_before), "another raft");
        assert_eq!(support_sets(&document).len(), 1);

        // One undo step brings the old raft back, in force.
        assert!(document.undo().unwrap());
        assert!(Arc::ptr_eq(&run.support_leaf().unwrap().1, &mesh_before));
        let back = in_force_session(&run).expect("in force");
        assert_eq!(back.options().unwrap().raft, raft_before);
        assert!(document.redo().unwrap());
        assert!(Arc::ptr_eq(&run.support_leaf().unwrap().1, &mesh_after));

        // No raft, and the thickness of a raft: each redraws too.
        run.on_option(9, 2.0);
        let off = in_force_session(&run).expect("in force");
        assert_eq!(off.options().unwrap().raft, RaftMode::Off);
        assert_eq!(tips(&off), tips(&before));
        run.on_option(9, 0.0);
        run.on_option(10, 1.2);
        let thick = in_force_session(&run).expect("in force");
        assert!((thick.options().unwrap().raft_thickness_mm - 1.2).abs() < 1e-6);
        assert_eq!(tips(&thick), tips(&before));
    });
}
