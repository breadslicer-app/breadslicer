/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Headless tests of the Orient tool (#182): Apply is one Undo step, a
//! preview never reaches the history and goes back exactly, a cancelled or
//! stale search moves nothing, and a good pose stays.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, Vertex};
use sc_orient::SearchMode;
use sc_scene::node::LeafPayload;
use sc_scene::{NodeId, Scene};
use sc_scene_edit::SceneEdit;
use slint::Model;

use super::*;
use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
use crate::selection::{LeafId, Selection};
use crate::test_support::slint_thread::on_fresh_slint_thread;

/// A 20 mm cube.
fn cube() -> IndexedMesh {
    let mut cube = sc_mesh_fixtures::unit_cube_mesh();
    for v in &mut cube.vertices {
        *v = Vertex::new(v.as_array().map(|c| c * 20.0));
    }
    cube
}

/// The cube, tipped over on no face, 30 mm above the plate.
fn tipped() -> Transform3D {
    Transform3D::Affine(
        AffineTransform3D::from_translation([60.0, 40.0, 30.0])
            .compose(&AffineTransform3D::from_rotation_x(0.6))
            .compose(&AffineTransform3D::from_rotation_y(0.4)),
    )
}

/// A node id, for a target outside any scene.
fn some_leaf() -> NodeId {
    Scene::new()
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(cube())),
        )
        .unwrap()
}

struct Fixture {
    window: MainWindow,
    document: Rc<SceneDocument>,
    run: Rc<OrientRun>,
    leaf: NodeId,
}

/// A window with the tipped cube selected and the Orient tool installed.
fn fixture() -> Fixture {
    let window = MainWindow::new().expect("the headless platform creates a main window");
    let ctrl = Rc::new(ViewportController::new_for_test());
    let document = ctrl.document();
    let mut graph = Scene::new();
    let leaf = graph
        .add_leaf(None, tipped(), LeafPayload::Mesh(Arc::new(cube())))
        .unwrap();
    graph.add_root(leaf).unwrap();
    document.install_graph(graph, PathBuf::from("cube.stl"));
    document.set_selection(Selection::One(LeafId(leaf)));
    let supports = DfSupportRun::install(
        &window,
        PreferencesBus::new(PreferencesSnapshot::for_tests()),
        document.clone(),
        ctrl.dialogs(),
        ctrl.clone(),
    );
    let run = OrientRun::install(&window, document.clone(), ctrl.dialogs(), ctrl, supports);
    Fixture {
        window,
        document,
        run,
        leaf,
    }
}

/// Waits for the worker to hand its result over, then takes it as the
/// event loop would.
fn land(run: &OrientRun) {
    let start = Instant::now();
    while run
        .slot
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_none()
    {
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "the worker hangs"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    run.finish();
}

/// Opens the tool (the analysis is built) and runs a search to the end.
fn searched(f: &Fixture, mode: SearchMode) {
    if !f.run.open.get() {
        f.run.toggle();
        assert!(f.window.get_orient_open());
        land(&f.run);
        assert!(f.run.analysis.borrow().is_some(), "built on opening");
    }
    f.run.search(mode);
    assert!(f.window.get_orient_running());
    land(&f.run);
    assert!(!f.window.get_orient_running());
}

fn local(f: &Fixture) -> Transform3D {
    f.run.local_of(f.leaf).expect("the leaf")
}

fn options(f: &Fixture) -> usize {
    f.window.get_orient_options().row_count()
}

fn afters(f: &Fixture) -> Vec<Transform3D> {
    f.run
        .shown
        .borrow()
        .as_ref()
        .map(|s| s.afters.clone())
        .unwrap_or_default()
}

/// The lowest point and the footprint centre of the model, in world mm.
fn bounds(f: &Fixture) -> ([f64; 3], [f64; 3]) {
    f.document
        .with_scene(|spatial| {
            let world = affine(&spatial.leaf(f.leaf)?.world_transform());
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for v in &cube().vertices {
                let p = world.apply_to_vertex(*v).coords().map(f64::from);
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            Some((lo, hi))
        })
        .flatten()
        .expect("the model")
}

#[test]
fn applying_an_option_is_one_undo_step() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        let before = local(&f);
        let (lo0, hi0) = bounds(&f);
        searched(&f, SearchMode::Quick);
        assert_eq!(options(&f), 3);
        assert_eq!(f.window.get_orient_selected(), 0, "the best is picked");
        assert!(!f.document.can_undo(), "searching records nothing");
        assert_eq!(local(&f), before, "searching moves nothing");
        let after = afters(&f)[0].clone();
        assert_ne!(after, before, "a tipped cube is turned");

        assert!(f.run.apply());
        assert_eq!(local(&f), after);
        assert_eq!(options(&f), 0, "the options go once applied");
        // Turned about the centre of its footprint, the lift kept.
        let (lo1, hi1) = bounds(&f);
        assert!((lo1[2] - lo0[2]).abs() < 1e-3, "{} → {}", lo0[2], lo1[2]);
        for k in 0..2 {
            let (c0, c1) = ((lo0[k] + hi0[k]) / 2.0, (lo1[k] + hi1[k]) / 2.0);
            assert!((c0 - c1).abs() < 1e-3, "{c0} → {c1}");
        }

        // One Undo takes it all back, exactly; Redo brings it again.
        assert!(f.document.undo().unwrap());
        assert_eq!(local(&f), before);
        assert!(!f.document.can_undo());
        assert!(f.document.redo().unwrap());
        assert_eq!(local(&f), after);
    });
}

#[test]
fn hovering_shows_an_option_without_history_and_it_goes_back_exactly() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        let before = local(&f);
        searched(&f, SearchMode::Quick);
        let afters = afters(&f);

        f.run.hover(1, true);
        assert_eq!(local(&f), afters[1], "the option shows on the model");
        assert!(f.document.has_live_preview());
        assert!(!f.document.can_undo(), "a preview records nothing");
        f.run.hover(2, true);
        assert_eq!(local(&f), afters[2]);
        f.run.hover(1, false);
        assert_eq!(local(&f), afters[2], "leaving another card changes nothing");
        f.run.hover(2, false);
        assert_eq!(local(&f), before, "back exactly");
        assert!(!f.document.has_live_preview());

        // Closing the tool (Esc, another tool, another tab) puts it back.
        f.run.hover(0, true);
        f.run.close();
        assert_eq!(local(&f), before);
        assert!(!f.window.get_orient_open());
        assert!(!f.document.can_undo());
    });
}

#[test]
fn undo_or_a_save_during_a_preview_sees_the_scene_as_recorded() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        let before = local(&f);
        // Something to undo: a move along X.
        let moved = Transform3D::Affine(AffineTransform3D::from_translation([5.0, 0.0, 0.0]))
            .compose(&before);
        let edit = SceneEdit::SetLocalTransform {
            node_id: f.leaf,
            before: before.clone(),
            after: moved.clone(),
        };
        f.document.apply_edit(edit.clone()).unwrap();
        f.document.push_edit(edit);
        searched(&f, SearchMode::Quick);
        f.run.hover(0, true);
        assert_ne!(local(&f), moved);
        // Undo first puts the preview back, then undoes the move.
        assert!(f.document.undo().unwrap());
        assert_eq!(local(&f), before);
        assert!(!f.document.has_live_preview());

        // A save takes the recorded pose, never the preview.
        assert!(f.document.redo().unwrap());
        searched(&f, SearchMode::Quick);
        f.run.hover(0, true);
        let saved = crate::scene_save::prepare_save_for_target(
            &f.document,
            PathBuf::from("cube.bread"),
            &crate::print_selection::PrintDefaults::default(),
        );
        assert!(saved.is_some());
        assert_eq!(local(&f), moved, "the save put the pose back first");
    });
}

#[test]
fn a_result_for_a_changed_scene_is_dropped() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        f.run.toggle();
        land(&f.run);
        let before = local(&f);
        f.run.search(SearchMode::Quick);
        // The scene changes while the worker searches.
        let moved = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 7.0, 0.0]))
            .compose(&before);
        let edit = SceneEdit::SetLocalTransform {
            node_id: f.leaf,
            before,
            after: moved.clone(),
        };
        f.document.apply_edit(edit.clone()).unwrap();
        f.document.push_edit(edit);
        land(&f.run);
        assert_eq!(options(&f), 0, "nothing to apply");
        assert!(f.run.shown.borrow().is_none());
        assert!(f.window.get_orient_status().contains("changed"));
        assert_eq!(local(&f), moved, "the model stays where the person put it");
        assert!(!f.run.apply());
        assert_eq!(local(&f), moved);
    });
}

#[test]
fn a_cancelled_search_moves_nothing() {
    // The worker stops at once.
    let target = Target {
        leaf: some_leaf(),
        mesh: Arc::new(cube()),
        local: Transform3D::Identity,
        world: AffineTransform3D::identity(),
        parent_inverse: Transform3D::Identity,
        revision: SceneRevision::from_top_edit_id(None),
    };
    let worked = work(
        &target,
        None,
        Some(OrientContext::new(Preset::Balanced)),
        &AtomicBool::new(true),
        &mut |_| {},
    );
    assert!(worked.analysis.is_err());
    assert!(matches!(worked.found, Some(Err(Cancelled))));

    // The tool lands a cancelled search as nothing at all.
    on_fresh_slint_thread(|| {
        let f = fixture();
        let before = local(&f);
        f.run.toggle();
        land(&f.run);
        f.run.search(SearchMode::Full);
        f.run.cancel();
        let start = Instant::now();
        while f.run.slot.lock().unwrap().is_none() {
            assert!(start.elapsed() < Duration::from_secs(120));
            std::thread::sleep(Duration::from_millis(5));
        }
        // However far the worker got, the run was cancelled.
        let id = f.run.job.borrow().as_ref().unwrap().id;
        let analysis = f.run.analysis.borrow().as_ref().unwrap().1.clone();
        *f.run.slot.lock().unwrap() = Some((
            id,
            Ok(Worked {
                analysis: Ok(analysis),
                found: Some(Err(Cancelled)),
            }),
        ));
        f.run.finish();
        assert!(!f.window.get_orient_running());
        assert_eq!(options(&f), 0);
        assert!(f.window.get_orient_status().contains("cancelled"));
        assert_eq!(local(&f), before);
        assert!(!f.document.can_undo());
    });
}

#[test]
fn a_pose_that_is_already_the_best_stays() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        searched(&f, SearchMode::Quick);
        assert!(f.run.apply());
        let applied = local(&f);
        // Searching the result again finds nothing better.
        searched(&f, SearchMode::Quick);
        assert!(
            f.window.get_orient_status().contains("already the best"),
            "{}",
            f.window.get_orient_status()
        );
        assert_eq!(f.window.get_orient_selected(), -1, "nothing picked");
        assert!(!f.run.apply(), "nothing to apply until an option is picked");
        assert_eq!(local(&f), applied);
        // A Full search reuses the analysis and agrees.
        let analysis = f.run.analysis.borrow().as_ref().unwrap().1.clone();
        searched(&f, SearchMode::Full);
        assert!(Arc::ptr_eq(
            &analysis,
            &f.run.analysis.borrow().as_ref().unwrap().1
        ));
        assert!(f.window.get_orient_status().contains("already the best"));
        assert_eq!(
            f.window.get_orient_options().row_data(0).unwrap().mode,
            "Full"
        );
    });
}

#[test]
fn a_new_objective_runs_the_last_search_again() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        searched(&f, SearchMode::Full);
        f.run.on_option(0, 2.0);
        assert_eq!(f.run.settings.get().preset, Preset::LowerForce);
        assert!(f.window.get_orient_running(), "searching again");
        assert_eq!(
            f.run.job.borrow().as_ref().unwrap().mode,
            Some(SearchMode::Full)
        );
        land(&f.run);
        assert_eq!(options(&f), 3);
        // A setting that does not change leaves the options as they are.
        f.run.on_option(0, 2.0);
        assert!(!f.window.get_orient_running());
    });
}

#[test]
fn another_selection_drops_the_options() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        searched(&f, SearchMode::Quick);
        f.document.set_selection(Selection::None);
        f.run.on_scene_changed();
        assert_eq!(options(&f), 0);
        assert!(!f.window.get_orient_open(), "no model, no tool");
    });
}

#[test]
fn the_settings_build_the_search_context() {
    let s = OrientSettings {
        preset: Preset::Faster,
        max_tilt_deg: 40.0,
        align_to_plate: true,
        weight_supports: 2.0,
        weight_force: 0.5,
        weight_height: 1.0,
    };
    let ctx = s.context(SearchMode::Full, 50.0, 12.0, Some([100.0, 60.0, 150.0]));
    let w = Preset::Faster.weights();
    assert_eq!(ctx.mode, SearchMode::Full);
    assert_eq!(ctx.max_tilt_deg, Some(40.0));
    assert!(ctx.align_to_plate);
    assert_eq!(ctx.overhang_angle_deg, 50.0);
    assert_eq!(ctx.area_per_tip_mm2, 12.0);
    assert_eq!(ctx.build_volume_mm, Some([100.0, 60.0, 150.0]));
    assert_eq!(ctx.weights.tip, 2.0 * w.tip);
    assert_eq!(ctx.weights.peel_per_n, 0.5 * w.peel_per_n);
    assert_eq!(ctx.weights.height_per_mm, w.height_per_mm);
    // 180° allows any pose.
    let any = OrientSettings::default().context(SearchMode::Quick, 45.0, 10.0, None);
    assert_eq!(any.max_tilt_deg, None);
}

/// Times the tool's own worker on a real model: the analysis, then Quick and
/// Full on it, and the poses of the options.
/// `ORIENT_MODEL=<stl> cargo test -p sc-slicer-gui --release orient_timing -- --ignored --nocapture`
#[test]
#[ignore = "needs ORIENT_MODEL, a real model"]
fn orient_timing() {
    let path = std::env::var("ORIENT_MODEL").expect("ORIENT_MODEL");
    let loaded = sc_mesh_formats::load_mesh_from_path(
        std::path::Path::new(&path),
        None,
        Some(sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES),
    )
    .expect("the model opens");
    let flat = sc_mesh_formats::scene_writer::flatten_graph_to_mesh(&loaded.graph).unwrap();
    let target = Target {
        leaf: some_leaf(),
        mesh: Arc::new(flat.mesh),
        local: Transform3D::Identity,
        world: AffineTransform3D::identity(),
        parent_inverse: Transform3D::Identity,
        revision: SceneRevision::from_top_edit_id(None),
    };
    let cancel = AtomicBool::new(false);
    for round in 0..2 {
        let t = Instant::now();
        let prepared = work(&target, None, None, &cancel, &mut |_| {});
        let analysis = prepared.analysis.unwrap();
        let prepare = t.elapsed();
        for mode in [SearchMode::Quick, SearchMode::Full] {
            let ctx =
                OrientSettings::default().context(mode, 45.0, 10.0, Some([153.36, 77.76, 165.0]));
            let t = Instant::now();
            let worked = work(
                &target,
                Some(analysis.clone()),
                Some(ctx),
                &cancel,
                &mut |_| {},
            );
            let found = worked.found.unwrap().unwrap();
            println!(
                "round {round}: {} triangles, prepare {:.0} ms, {mode:?} {:.0} ms (search {:.0} ms), keep {}, best ≈ {:.0} tips, peak {:.1} N",
                analysis.triangle_counts().0,
                prepare.as_secs_f64() * 1e3,
                t.elapsed().as_secs_f64() * 1e3,
                found.outcome.timings.search_ms(),
                found.outcome.keep_current,
                found.outcome.options[0].cost.estimated_tips(),
                found.outcome.options[0].cost.peel.peak_force_n,
            );
        }
    }
}
