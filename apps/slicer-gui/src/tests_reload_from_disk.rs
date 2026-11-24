//! Reload from disk (#721).

use std::rc::Rc;
use std::time::Duration;

use super::*;
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::tests_model_import::{finish_loads, leaves, write_box};

/// A window with `goblin.stl` imported, and its reload driver.
struct Fixture {
    window: MainWindow,
    ctrl: Rc<ViewportController>,
    run: Rc<ReloadRun>,
    hollow: Rc<HollowRun>,
    path: PathBuf,
    _dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_box(dir.path(), "goblin.stl", [10.0, 10.0, 10.0]);
    let window = MainWindow::new().expect("a window");
    let ctrl = Rc::new(ViewportController::new_for_test_with_ui(&window));
    ctrl.register_document_listener();
    // Into a scene, as an import.
    ctrl.document()
        .install_graph(sc_scene::Scene::new(), dir.path().join("scene.bread"));
    ctrl.open_models(vec![path.clone()]);
    finish_loads(&ctrl);
    let document = ctrl.document();
    let hollow = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
    let bus = PreferencesBus::new(crate::preferences_bus::PreferencesSnapshot::for_tests());
    let run = ReloadRun::install(&window, document, hollow.clone(), bus, ctrl.clone());
    run.timer.stop();
    Fixture {
        window,
        ctrl,
        run,
        hollow,
        path,
        _dir: dir,
    }
}

/// The model's mesh width along X.
fn width(f: &Fixture) -> f32 {
    let document = f.ctrl.document();
    let leaf = leaves(&document)[0];
    document
        .with_scene(|spatial| {
            let mesh = spatial.graph().leaf_mesh_arc(leaf).unwrap().clone();
            let xs = mesh.vertices.iter().map(|v| v.coords()[0]);
            xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min)
        })
        .unwrap()
}

fn world(f: &Fixture) -> sc_mesh_core::transform::Transform3D {
    let document = f.ctrl.document();
    let leaf = leaves(&document)[0];
    document
        .with_scene(|spatial| spatial.graph().world_transform_of(leaf).unwrap())
        .unwrap()
}

#[test]
fn a_model_reloads_from_its_file_at_its_pose_in_one_undo_step() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        let document = f.ctrl.document();
        let model = leaves(&document)[0];
        assert_eq!(document.reloadable_file(model), Some(f.path.clone()));
        let pose = world(&f);
        assert!((width(&f) - 10.0).abs() < 1e-3);
        // Edited and saved again in another program.
        write_box(f._dir.path(), "goblin.stl", [20.0, 10.0, 10.0]);
        f.run.reload(&[model], true);
        f.run.land_now();
        assert!((width(&f) - 20.0).abs() < 1e-3, "{}", width(&f));
        assert_eq!(world(&f), pose, "at the same pose");
        assert!(
            f.window.get_status_notice().contains("Reloaded goblin.stl"),
            "{}",
            f.window.get_status_notice()
        );
        assert!(document.undo().expect("undoes"));
        assert!((width(&f) - 10.0).abs() < 1e-3, "one undo takes it back");
    });
}

#[test]
fn a_file_saved_again_reloads_on_its_own_once_it_holds_still() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        // The first look notes the file as it is.
        f.run.watch_now();
        std::thread::sleep(Duration::from_millis(30));
        write_box(f._dir.path(), "goblin.stl", [20.0, 10.0, 10.0]);
        // Seen changed: not read half written; the next look reads it.
        f.run.watch_now();
        assert!(f.run.jobs.borrow().is_empty());
        f.run.watch_now();
        f.run.land_now();
        assert!((width(&f) - 20.0).abs() < 1e-3, "{}", width(&f));
        // Nothing changed since: nothing more.
        f.run.watch_now();
        f.run.watch_now();
        assert!(f.run.jobs.borrow().is_empty());
    });
}

#[test]
fn a_file_gone_leaves_its_model_and_says_why() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        let document = f.ctrl.document();
        let model = leaves(&document)[0];
        std::fs::remove_file(&f.path).expect("removed");
        f.run.reload(&[model], true);
        f.run.land_now();
        assert!((width(&f) - 10.0).abs() < 1e-3, "the model stays");
        assert!(
            f.window
                .get_status_notice()
                .contains("could not be read again"),
            "{}",
            f.window.get_status_notice()
        );
    });
}

#[test]
fn a_file_that_gives_other_models_now_leaves_them_as_they_are() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        let document = f.ctrl.document();
        let model = leaves(&document)[0];
        // Now a project, not the model's file: nothing to take from it.
        std::fs::write(&f.path, b"solid nothing\nendsolid nothing\n").expect("written");
        f.run.reload(&[model], true);
        f.run.land_now();
        assert!((width(&f) - 10.0).abs() < 1e-3);
        assert!(!f.window.get_status_notice().is_empty());
    });
}

#[test]
fn a_hollowed_model_is_hollowed_again_with_its_settings() {
    on_fresh_slint_thread(|| {
        let f = fixture();
        let document = f.ctrl.document();
        let model = leaves(&document)[0];
        document.set_selection(crate::selection::Selection::One(crate::selection::LeafId(
            model,
        )));
        f.hollow.hollow_now(1.5, &[]);
        let hollowed = document
            .with_scene(|spatial| spatial.graph().leaf_mesh_arc(model).cloned())
            .flatten()
            .expect("the hollow");
        assert!(f.hollow.record_of(&hollowed).is_some(), "hollowed here");
        write_box(f._dir.path(), "goblin.stl", [20.0, 10.0, 10.0]);
        f.run.reload(&[model], true);
        f.run.land_now();
        assert!(f.window.get_hollow_running(), "hollowed again");
        assert!(
            f.window.get_status_notice().contains("hollowing it again"),
            "{}",
            f.window.get_status_notice()
        );
    });
}
