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
use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};

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
    ctrl: Rc<ViewportController>,
    leaf: NodeId,
}

/// A window with the tipped cube selected and the Orient tool installed.
fn fixture() -> Fixture {
    fixture_at(tipped())
}

/// The tipped cube with its lowest corner on the plate.
fn resting() -> Transform3D {
    let world = affine(&tipped());
    let lowest = cube()
        .vertices
        .iter()
        .map(|v| f64::from(world.apply_to_vertex(*v).coords()[2]))
        .fold(f64::INFINITY, f64::min);
    Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, -lowest]).compose(&world))
}

/// A window with the cube at `pose`, selected, and the Orient tool installed.
fn fixture_at(pose: Transform3D) -> Fixture {
    fixture_with(cube(), pose)
}

/// A window with `mesh` at `pose`, selected, and the Orient tool installed.
fn fixture_with(mesh: IndexedMesh, pose: Transform3D) -> Fixture {
    let window = MainWindow::new().expect("the headless platform creates a main window");
    let ctrl = Rc::new(ViewportController::new_for_test());
    let document = ctrl.document();
    let mut graph = Scene::new();
    let leaf = graph
        .add_leaf(None, pose, LeafPayload::Mesh(Arc::new(mesh)))
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
    let run = OrientRun::install(
        &window,
        document.clone(),
        ctrl.dialogs(),
        ctrl.clone(),
        supports,
    );
    Fixture {
        window,
        document,
        run,
        ctrl,
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
            let mesh = spatial.graph().leaf_mesh_arc(f.leaf)?;
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for v in &mesh.vertices {
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
        plate: None,
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
        plate: None,
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

/// The lowest point of the model, from its vertices, in world mm.
fn lowest(f: &Fixture) -> f64 {
    bounds(f).0[2]
}

/// The lowest point of the model as the viewport draws it: in f32, with the
/// matrix the GPU gets (`mesh_render::scene_upload::leaf_world_box`). Below 0
/// the plate shows the model through it.
fn drawn_lowest(f: &Fixture) -> f32 {
    let (world, mesh) = f
        .document
        .with_scene(|spatial| {
            Some((
                affine(&spatial.leaf(f.leaf)?.world_transform()),
                spatial.graph().leaf_mesh_arc(f.leaf)?.clone(),
            ))
        })
        .flatten()
        .expect("the model");
    let (l, t) = (world.linear(), world.translation());
    let m = glam::Mat4::from_cols(
        glam::Vec4::new(l[0][0] as f32, l[1][0] as f32, l[2][0] as f32, 0.0),
        glam::Vec4::new(l[0][1] as f32, l[1][1] as f32, l[2][1] as f32, 0.0),
        glam::Vec4::new(l[0][2] as f32, l[1][2] as f32, l[2][2] as f32, 0.0),
        glam::Vec4::new(t[0] as f32, t[1] as f32, t[2] as f32, 1.0),
    );
    mesh.vertices
        .iter()
        .map(|v| m.transform_point3(glam::Vec3::from(v.coords())).z)
        .fold(f32::INFINITY, f32::min)
}

/// The model's box as the viewport draws it (f32, with the GPU's matrix),
/// and whether the viewport finds it inside the build envelope: outside, it
/// tints the model and marks the walls it crosses.
fn drawn_inside(f: &Fixture) -> (bool, sc_mesh_core::Aabb) {
    let (world, mesh) = f
        .document
        .with_scene(|spatial| {
            Some((
                affine(&spatial.leaf(f.leaf)?.world_transform()),
                spatial.graph().leaf_mesh_arc(f.leaf)?.clone(),
            ))
        })
        .flatten()
        .expect("the model");
    let (l, t) = (world.linear(), world.translation());
    let m = glam::Mat4::from_cols(
        glam::Vec4::new(l[0][0] as f32, l[1][0] as f32, l[2][0] as f32, 0.0),
        glam::Vec4::new(l[0][1] as f32, l[1][1] as f32, l[2][1] as f32, 0.0),
        glam::Vec4::new(l[0][2] as f32, l[1][2] as f32, l[2][2] as f32, 0.0),
        glam::Vec4::new(t[0] as f32, t[1] as f32, t[2] as f32, 1.0),
    );
    let mut aabb = sc_mesh_core::Aabb::empty();
    for v in &mesh.vertices {
        let p = m.transform_point3(glam::Vec3::from(v.coords()));
        aabb.extend(Vertex::new([p.x, p.y, p.z]));
    }
    (f.ctrl.build_volume().volume().contains(&aabb), aabb)
}

/// A 60 × 10 × 6 mm bar.
fn bar() -> IndexedMesh {
    let mut bar = sc_mesh_fixtures::unit_cube_mesh();
    for v in &mut bar.vertices {
        let [x, y, z] = v.as_array();
        *v = Vertex::new([x * 60.0, y * 10.0, z * 6.0]);
    }
    bar
}

/// Near the edge of the plate, a turn to lie square to it, or a tilt, can
/// reach past the edge although the model fits the plate: the options shift
/// it back in, in the preview and once applied.
#[test]
fn options_near_the_edge_stay_on_the_plate() {
    on_fresh_slint_thread(|| {
        let probe = fixture();
        let hi = probe.ctrl.build_volume().envelope_max_mm();
        // Turned 60°, resting on the plate, 26 mm in from the +X edge:
        // inside, but lying along X it would reach 4 mm past the edge.
        let pose = Transform3D::Affine(
            AffineTransform3D::from_translation([f64::from(hi.x) - 26.0, 0.0, 3.0])
                .compose(&AffineTransform3D::from_rotation_z(60f64.to_radians())),
        );
        let f = fixture_with(bar(), pose);
        assert!(drawn_inside(&f).0, "inside to start with");
        f.run.on_option(2, 1.0);
        for mode in [SearchMode::Quick, SearchMode::Full] {
            searched(&f, mode);
            for i in 0..options(&f) {
                f.run.hover(i as i32, true);
                let (inside, aabb) = drawn_inside(&f);
                assert!(inside, "{mode:?} preview {i}: {aabb:?}");
                assert!(lowest(&f).abs() < 1e-3, "{mode:?} preview {i} floats");
                f.run.hover(i as i32, false);
            }
            f.run.pick(0);
            assert!(f.run.apply());
            assert!(
                drawn_inside(&f).0,
                "{mode:?} applied: {:?}",
                drawn_inside(&f).1
            );
            drop_to_plate(&f);
            assert!(
                drawn_inside(&f).0,
                "{mode:?} dropped: {:?}",
                drawn_inside(&f).1
            );
            assert!(f.document.undo().unwrap());
        }
    });
}

/// Drop to plate on the selected model, as the Transform inspector does it.
fn drop_to_plate(f: &Fixture) {
    let change = f
        .document
        .with_scene(|spatial| {
            let query = spatial.leaf(f.leaf)?;
            let after = crate::viewport::numeric_transform::apply(
                &query,
                crate::viewport::numeric_transform::NumericEdit::DropToPlate,
            )?;
            Some((query.local_transform(), after))
        })
        .flatten()
        .expect("Drop to plate applies");
    let edit = SceneEdit::SetLocalTransform {
        node_id: f.leaf,
        before: change.0,
        after: change.1,
    };
    f.document.apply_edit(edit.clone()).unwrap();
    f.document.push_edit(edit);
}

/// Aligned to the plate, every option of a model resting on the plate keeps
/// it on the plate: in the preview, once applied, and after Drop to plate.
#[test]
fn aligned_options_rest_on_the_plate() {
    on_fresh_slint_thread(|| aligned_options_rest(fixture_at(resting())));
}

/// The same on a real model resting on the plate:
/// `ORIENT_MODEL=<stl> cargo test -p sc-slicer-gui --release aligned_real -- --ignored`
#[test]
#[ignore = "needs ORIENT_MODEL, a real model"]
fn aligned_real_model_rests_on_the_plate() {
    let path = std::env::var("ORIENT_MODEL").expect("ORIENT_MODEL");
    let loaded = sc_mesh_formats::load_mesh_from_path(
        std::path::Path::new(&path),
        None,
        Some(sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES),
    )
    .expect("the model opens");
    let mesh = sc_mesh_formats::scene_writer::flatten_graph_to_mesh(&loaded.graph)
        .unwrap()
        .mesh;
    let low = mesh
        .vertices
        .iter()
        .map(|v| f64::from(v.coords()[2]))
        .fold(f64::INFINITY, f64::min);
    let pose = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, -low]));
    on_fresh_slint_thread(move || aligned_options_rest(fixture_with(mesh, pose)));
}

fn aligned_options_rest(f: Fixture) {
    {
        assert!(lowest(&f).abs() < 1e-4, "{}", lowest(&f));
        f.run.on_option(2, 1.0);
        assert!(f.run.settings.get().align_to_plate);
        for mode in [SearchMode::Quick, SearchMode::Full] {
            searched(&f, mode);
            let count = options(&f);
            assert_eq!(count, 3);
            for i in 0..count {
                f.run.hover(i as i32, true);
                assert!(
                    lowest(&f).abs() < 1e-3,
                    "{mode:?} preview {i}: {}",
                    lowest(&f)
                );
                assert!(
                    drawn_lowest(&f) >= 0.0,
                    "{mode:?} preview {i} drawn: {}",
                    drawn_lowest(&f)
                );
                f.run.hover(i as i32, false);
            }
            f.run.pick(2);
            assert!(f.run.apply());
            assert!(lowest(&f).abs() < 1e-3, "{mode:?} applied: {}", lowest(&f));
            assert!(
                drawn_lowest(&f) >= 0.0,
                "{mode:?} applied drawn: {}",
                drawn_lowest(&f)
            );
            drop_to_plate(&f);
            assert!(lowest(&f).abs() < 1e-3, "{mode:?} dropped: {}", lowest(&f));
            assert!(
                drawn_lowest(&f) >= 0.0,
                "{mode:?} dropped drawn: {}",
                drawn_lowest(&f)
            );
        }
    }
}

/// Where `label` is drawn in `window`: its top and bottom, in logical
/// pixels. The first element with that accessible label and a size.
fn drawn_span(window: &MainWindow, label: &str) -> (f32, f32) {
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(window, label)
        .find(|e| e.size().height > 0.0)
        .map(|e| {
            let y = e.absolute_position().y;
            (y, y + e.size().height)
        })
        .unwrap_or_else(|| panic!("{label} is drawn"))
}

/// With Advanced open and three options, the card grows to hold everything:
/// its last row (Apply, Close) ends inside the card, above its padding, and
/// the next card starts below it.
#[test]
fn the_card_holds_its_last_row_with_advanced_open_and_three_options() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        f.window
            .show()
            .expect("the headless platform shows the window");
        f.window
            .window()
            .set_size(slint::LogicalSize::new(1400.0, 1400.0));
        f.window.set_has_scene(true);
        f.window.set_selection_valid(true);
        searched(&f, SearchMode::Quick);
        assert_eq!(options(&f), 3);
        render_frames();
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&f.window, "Advanced")
            .next()
            .expect("the Advanced row")
            .invoke_accessible_default_action();
        render_frames();
        // Advanced is open: its fields and its note are drawn.
        drawn_span(&f.window, "Align to the plate");
        let (_, note_bottom) = drawn_span(
            &f.window,
            "180° allows any pose, upside down included. Without aligning, an option turns about the vertical only to fit the plate.",
        );
        let card = i_slint_backend_testing::ElementHandle::find_by_element_type_name(
            &f.window,
            "OrientCard",
        )
        .next()
        .expect("the Orient card");
        let card_bottom = card.absolute_position().y + card.size().height;
        let (apply_top, apply_bottom) = drawn_span(&f.window, "Apply A");
        let (_, close_bottom) = drawn_span(&f.window, "Close");
        let (objects_top, _) = drawn_span(&f.window, "OBJECTS");
        assert!(note_bottom < apply_top);
        // The card body has 12 px of padding below its last row.
        for (name, bottom) in [("Apply", apply_bottom), ("Close", close_bottom)] {
            assert!(
                bottom <= card_bottom - 12.0 + 0.5,
                "{name} ends at {bottom}, past the card's content (to {})",
                card_bottom - 12.0
            );
        }
        assert!(objects_top >= card_bottom, "the next card overlaps");
    });
}
