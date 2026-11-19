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

/// One raft for all (#556) is a raft of lines: the box turns the Supports
/// tab's raft to lines, drawing the sets again on it, and another raft
/// there turns the box off.
#[test]
fn one_raft_for_all_turns_to_lines_and_another_raft_turns_it_off() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run) = supported_slab(&window);
        let ctrl = Rc::new(ViewportController::new_for_test());
        let arrange = crate::arrange_run::ArrangeRun::install(
            &window,
            document.clone(),
            ctrl.dialogs(),
            ctrl.clone(),
        );
        assert!(!window.get_arrange_one_raft(), "off by default");
        assert!(!arrange.options_now().one_raft);
        let before = in_force_session(&run).expect("editable");
        assert_ne!(before.options().unwrap().raft, RaftMode::Lines);
        let mesh_before = run.support_leaf().expect("the supports").1;

        let preset = (
            window.get_support_preset_index(),
            window.get_support_based_on(),
        );
        window.invoke_arrange_option_changed(5, 1.0);
        assert!(window.get_arrange_one_raft());
        assert_eq!(window.get_support_raft_index(), 1, "lines in the tab");
        assert_eq!(
            (
                window.get_support_preset_index(),
                window.get_support_based_on()
            ),
            preset,
            "the preset stays as it was"
        );
        let lines = in_force_session(&run).expect("still in force");
        assert_eq!(lines.options().unwrap().raft_drawn(), RaftMode::Lines);
        assert!(lines.stands_on_one_raft());
        assert_eq!(tips(&lines), tips(&before), "the supports did not move");
        let mesh_lines = run.support_leaf().expect("the supports").1;
        assert!(
            !Arc::ptr_eq(&mesh_lines, &mesh_before),
            "drawn again on lines"
        );

        // Arrange packs by what stands above the rafts, tighter, with the
        // card's gap.
        let options = arrange.options_now();
        assert!(options.raft_top_mm > 0.0, "{options:?}");
        let settings = options.settings();
        assert_eq!(settings.gap, f64::from(options.gap_mm), "{settings:?}");
        assert!(settings.tight, "{settings:?}");
        assert!(
            settings.free_below > f64::from(options.raft_top_mm),
            "{settings:?}"
        );

        // Another raft in the Supports tab turns the box off.
        run.on_option(9, 0.0);
        assert!(!window.get_arrange_one_raft());
        assert!(!arrange.options_now().one_raft);
        assert_eq!(arrange.options_now().settings().free_below, 0.0);

        // Lines picked again keep the box off; the box turns it back on.
        run.on_option(9, 1.0);
        assert!(!window.get_arrange_one_raft());
        window.invoke_arrange_option_changed(5, 1.0);
        assert!(window.get_arrange_one_raft());
        run.on_option(9, 1.0);
        assert!(window.get_arrange_one_raft(), "lines again keep it on");
    });
}

/// The raft leaves of `document`'s scene.
fn plate_rafts(document: &SceneDocument) -> Vec<(NodeId, Arc<sc_mesh_core::IndexedMesh>)> {
    document
        .with_scene(|spatial| {
            (0..)
                .map_while(|i| spatial.leaf_id_at_index(i))
                .filter_map(|leaf| {
                    let mesh = spatial.graph().leaf_mesh_arc(leaf)?;
                    sc_geom::df_supports::is_plate_raft(mesh).then(|| (leaf, mesh.clone()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Ticks the plate's raft until the scene stands still with it.
fn settle_rafts(document: &SceneDocument, run: &DfSupportRun) {
    let volume = run.ctrl.build_volume();
    for _ in 0..400 {
        let revision = document.scene_revision();
        run.plate_rafts.tick(document, &volume);
        std::thread::sleep(Duration::from_millis(5));
        run.plate_rafts.tick(document, &volume);
        if document.scene_revision() == revision && !run.plate_rafts.busy() {
            return;
        }
    }
    panic!("the raft never settled");
}

/// One raft for all (#556): the set is drawn again with no raft of its own,
/// and the plate shows one raft under it, in the same undo step; Undo takes
/// both back, Redo brings both.
#[test]
fn one_raft_for_all_lays_one_raft_under_the_sets_in_their_undo_step() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run) = supported_slab(&window);
        // On a raft of lines of its own first.
        run.on_option(9, 1.0);
        let own = run.support_leaf().expect("the supports").1;
        assert!(plate_rafts(&document).is_empty());
        let steps = document.undo_len();

        run.on_option(14, 1.0);
        let set = in_force_session(&run).expect("in force");
        assert!(set.stands_on_one_raft());
        let apart = run.support_leaf().expect("the supports").1;
        assert!(
            apart.faces().len() < own.faces().len(),
            "no raft of its own"
        );
        settle_rafts(&document, &run);
        let rafts = plate_rafts(&document);
        assert_eq!(rafts.len(), 1, "one raft on the plate");
        assert_eq!(document.undo_len(), steps + 1, "with the set, one step");

        // Settled: looking again draws nothing.
        let revision = document.scene_revision();
        settle_rafts(&document, &run);
        assert_eq!(document.scene_revision(), revision);

        assert!(document.undo().unwrap());
        assert!(plate_rafts(&document).is_empty(), "undone with the set");
        assert!(Arc::ptr_eq(&run.support_leaf().unwrap().1, &own));
        settle_rafts(&document, &run);
        assert!(
            plate_rafts(&document).is_empty(),
            "the set has its own raft"
        );
        assert!(document.redo().unwrap());
        assert_eq!(plate_rafts(&document).len(), 1, "redone with the set");
        settle_rafts(&document, &run);
        assert_eq!(plate_rafts(&document).len(), 1);
        assert!(Arc::ptr_eq(&plate_rafts(&document)[0].1, &rafts[0].1));

        // Off again: the set draws its raft, the plate's goes, one step.
        let steps = document.undo_len();
        run.on_option(14, 0.0);
        settle_rafts(&document, &run);
        assert!(plate_rafts(&document).is_empty());
        assert_eq!(document.undo_len(), steps + 1);
    });
}

/// The one raft for all is supports to the review (#606): a set with no
/// contacts, never a model standing in its own raft.
#[test]
fn the_review_reads_the_one_raft_for_all_as_supports() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run) = supported_slab(&window);
        run.on_option(9, 1.0);
        run.on_option(14, 1.0);
        settle_rafts(&document, &run);
        let rafts = plate_rafts(&document);
        assert_eq!(rafts.len(), 1, "one raft on the plate");
        let sets = document
            .with_scene(|spatial| {
                crate::support_persist::review_sets(spatial, &document.support_sessions())
            })
            .expect("a scene");
        let raft = sets
            .iter()
            .find(|set| set.leaf == rafts[0].0)
            .expect("the raft is read as supports");
        assert!(raft.contacts.is_empty(), "and takes no tip");
        assert_eq!(sets.len(), 2, "the set and the raft");
    });
}

/// A slice made for Fix All waits for the one raft for all to be drawn
/// again: the supports changed, the raft follows them in the background,
/// and a slice made before it lands would be out of date at once.
#[test]
fn the_raft_still_to_be_drawn_is_told_until_it_lands() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run) = supported_slab(&window);
        assert!(!run.rafts_pending(), "a raft of its own: nothing to wait for");
        run.on_option(9, 1.0);
        run.on_option(14, 1.0);
        assert!(run.rafts_pending(), "the plate's raft is still to be drawn");
        settle_rafts(&document, &run);
        assert_eq!(plate_rafts(&document).len(), 1);
        assert!(!run.rafts_pending(), "drawn: a slice can go");
    });
}

/// One model on one raft for all: the raft is that model's set's alone, and
/// rides along as the model is dragged, then stays where the move took it
/// (a tester saw it wait behind until the model was let go).
#[test]
fn the_raft_of_one_model_alone_moves_with_it_while_it_is_dragged() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run, leaves) = supported(&window, &[[20.0, 20.0, 8.0]]);
        let model = leaves[0];
        run.on_option(9, 1.0);
        run.on_option(14, 1.0);
        settle_rafts(&document, &run);
        let (raft, _) = plate_rafts(&document).pop().expect("one raft on the plate");
        let start = leaf_transform(&document, raft);
        let (at, there) = ([20.0, 20.0, 8.0], [27.0, 15.0, 8.0]);

        // Live: the raft rides along with the set.
        move_live(&document, model, at, there);
        assert_eq!(
            leaf_transform(&document, raft),
            translation([7.0, -5.0, 0.0]).compose(&start)
        );
        // Let go: it stays there, in the move's step, and is drawn again
        // where it stands.
        commit_move(&document, model, at, there);
        assert_eq!(
            leaf_transform(&document, raft),
            translation([7.0, -5.0, 0.0]).compose(&start)
        );
        let centre = |document: &SceneDocument| {
            document
                .with_scene(|s| {
                    let leaf = plate_rafts(document)[0].0;
                    let q = s.leaf(leaf)?;
                    let b = q.transformed_aabb(q.world_transform())?;
                    let (lo, hi) = (b.min().coords(), b.max().coords());
                    Some([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0])
                })
                .flatten()
                .expect("a raft")
        };
        let moved = centre(&document);
        settle_rafts(&document, &run);
        let drawn = centre(&document);
        assert!(
            (moved[0] - drawn[0]).abs() < 0.05 && (moved[1] - drawn[1]).abs() < 0.05,
            "carried {moved:?}, drawn again {drawn:?}"
        );
        assert!(document.undo().unwrap());
        assert_eq!(leaf_transform(&document, raft), start, "one step back");
    });
}
