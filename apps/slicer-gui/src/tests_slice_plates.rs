/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Slicing one plate of several (#547), through the window's own Slice press:
//! the active plate's models alone, moved to the printer's origin.

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Instant;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::node::LeafPayload;

use super::*;
use crate::WorkspaceSection;
use crate::build_volume::BuildVolume;
use crate::df_support_run::DfSupportRun;
use crate::hollow_run::HollowRun;
use crate::preferences_bus::PreferencesSnapshot;
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::viewport::ViewportController;

/// The longest a slice may take before the test calls it hung.
const PATIENCE: Duration = Duration::from_secs(300);

struct Window {
    window: MainWindow,
    ctrl: Rc<ViewportController>,
    document: Rc<SceneDocument>,
    slice: Rc<SliceRun>,
    /// The cube on the first plate and the one on the second.
    cubes: [NodeId; 2],
}

/// A cube `size` mm on a side standing on the plate at `x`, `y`.
fn cube_at(graph: &mut Scene, size: f64, x: f64, y: f64) -> NodeId {
    let place = Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
        [[size, 0.0, 0.0], [0.0, size, 0.0], [0.0, 0.0, size]],
        [x, y, size / 2.0],
    ));
    let leaf = graph
        .add_leaf(None, place, LeafPayload::Mesh(Arc::new(unit_cube_mesh())))
        .expect("inline leaf");
    graph.add_root(leaf).expect("root");
    leaf
}

/// A window with two plates: a 10 mm cube on the first, 50 mm left of its
/// centre, and a 20 mm cube in the middle of the second.
fn two_plates() -> Window {
    let window = MainWindow::new().expect("the headless platform creates a main window");
    let bus = PreferencesBus::new(PreferencesSnapshot {
        default_printer: Some(PrinterId::new("elegoo-saturn-4-ultra").expect("valid id")),
        default_resin: Some(MaterialId::new("standard-405").expect("valid id")),
        ..PreferencesSnapshot::for_tests()
    });
    let ctrl = Rc::new(ViewportController::new_for_test_with_ui(&window));
    let volume = BuildVolume::new(218.88, 122.88, 220.0);
    ctrl.set_build_volume(volume);
    let second = crate::plates::offset(&volume, 1);
    let mut graph = Scene::new();
    let first = cube_at(&mut graph, 10.0, -50.0, 0.0);
    let other = cube_at(&mut graph, 20.0, f64::from(second.x), f64::from(second.y));
    let document = ctrl.document();
    document.install_graph(graph, PathBuf::from("plates.bread"));
    ctrl.add_plate();
    let dialogs = ctrl.dialogs();
    dialogs.attach(&window);
    let repair = RepairRun::install(&window, document.clone(), dialogs.clone(), ctrl.clone());
    let supports = DfSupportRun::install(
        &window,
        bus.clone(),
        document.clone(),
        dialogs.clone(),
        ctrl.clone(),
    );
    let hollow = HollowRun::install(&window, document.clone(), dialogs.clone(), ctrl.clone());
    let review = ReviewPanel::install(
        &window,
        bus.clone(),
        document.clone(),
        ctrl.clone(),
        supports,
        hollow,
    );
    let slice = SliceRun::install(
        &window,
        bus,
        document.clone(),
        dialogs,
        review,
        repair,
        ctrl.clone(),
    );
    ctrl.set_section(WorkspaceSection::Slice);
    Window {
        window,
        ctrl,
        document,
        slice,
        cubes: [first, other],
    }
}

impl Window {
    /// Presses Slice and waits for the run's wake-up.
    fn press_slice(&self) {
        self.window.invoke_slice_requested();
        self.finish();
    }

    /// Waits for the slice in flight and hands its wake-up to the window.
    fn finish(&self) {
        assert!(self.window.get_slicing_in_progress(), "a slice runs");
        let start = Instant::now();
        while self
            .slice
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
        {
            assert!(start.elapsed() < PATIENCE, "the slice worker hangs");
            std::thread::sleep(Duration::from_millis(5));
        }
        self.window.invoke_slice_finished();
    }

    /// The box the outlines of the layer at `z_mm` take, `[min x, min y,
    /// max x, max y]`.
    fn outline_box(&self, z_mm: f64) -> Option<[f64; 4]> {
        let count = self.document.slice_layer_count()?;
        let layer = (0..count).find(|&k| {
            self.document
                .with_slice_layer(k, |layer| layer.z_mm() >= z_mm)
                .unwrap_or(false)
        })?;
        self.document
            .with_slice_layer(layer, |layer| {
                let mut found: Option<[f64; 4]> = None;
                for polygon in layer.polygons() {
                    for p in polygon.outer().vertices() {
                        let b = found.get_or_insert([p.x, p.y, p.x, p.y]);
                        *b = [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)];
                    }
                }
                found
            })
            .flatten()
    }
}

#[test]
fn the_active_plate_is_sliced_alone_at_the_printers_origin() {
    on_fresh_slint_thread(|| {
        let w = two_plates();
        w.document.set_active_plate(1);
        w.press_slice();
        let params = w.document.slice_params().expect("the slice landed");
        assert_eq!(params.plate, 1);
        // The second plate's cube alone, in the middle of the printer's plate.
        let low = w.outline_box(5.0).expect("ink at 5 mm");
        for (got, want) in low.iter().zip([-10.0, -10.0, 10.0, 10.0]) {
            assert!((got - want).abs() < 0.1, "{low:?}");
        }
        let high = w.outline_box(15.0).expect("ink at 15 mm: the taller cube");
        assert!((high[2] - 10.0).abs() < 0.1, "{high:?}");

        // Another plate made active: the slice held is the other plate's.
        w.document.set_active_plate(0);
        let current = SliceParams {
            plate: w.document.slice_plate(),
            ..params.clone()
        };
        assert_eq!(
            decide_re_slice(Some(&params), &current, false, false),
            ReSliceDecision::Changed
        );
        let _ = &w.ctrl;
    });
}

#[test]
fn the_first_plate_slices_where_it_stands() {
    on_fresh_slint_thread(|| {
        let w = two_plates();
        w.document.set_active_plate(0);
        w.press_slice();
        assert_eq!(w.document.slice_params().map(|p| p.plate), Some(0));
        // The 10 mm cube 50 mm left of the centre, and nothing higher.
        let low = w.outline_box(5.0).expect("ink at 5 mm");
        for (got, want) in low.iter().zip([-55.0, -5.0, -45.0, 5.0]) {
            assert!((got - want).abs() < 0.1, "{low:?}");
        }
        assert!(
            w.outline_box(15.0).is_none(),
            "the second plate's cube is not sliced"
        );
    });
}

#[test]
fn a_slice_of_every_plate_hears_when_each_lands() {
    on_fresh_slint_thread(|| {
        let w = two_plates();
        let landed = Rc::new(Cell::new(None));
        for plate in [1, 0] {
            w.document.set_active_plate(plate);
            let heard = landed.clone();
            w.slice.slice_then(move |ok| heard.set(Some(ok)));
            assert_eq!(landed.get(), None, "plate {plate} slices first");
            w.finish();
            assert_eq!(
                landed.take(),
                Some(PlateSliced::Sliced),
                "plate {plate} landed"
            );
            assert_eq!(w.document.slice_params().map(|p| p.plate), Some(plate));
        }
        // The plate held already: told at once.
        let heard = landed.clone();
        w.slice.slice_then(move |ok| heard.set(Some(ok)));
        assert_eq!(landed.take(), Some(PlateSliced::UpToDate), "up to date");
    });
}

/// An edit on another plate leaves the held slice current; one on its own
/// plate does not (#547).
#[test]
fn only_the_sliced_plate_makes_its_slice_old() {
    on_fresh_slint_thread(|| {
        let w = two_plates();
        w.document.set_active_plate(0);
        w.press_slice();
        assert!(!w.document.held_slice_changed());
        let nudge = |leaf| {
            let edit =
                crate::plate_layout::moved_by(&w.document, leaf, glam::Vec3::new(3.0, 0.0, 0.0))
                    .expect("a move");
            w.document.apply_edit(edit.clone()).expect("applies");
            w.document.push_edit(edit);
        };
        nudge(w.cubes[1]);
        assert!(!w.document.held_slice_changed(), "the other plate moved");
        nudge(w.cubes[0]);
        assert!(w.document.held_slice_changed(), "its own plate moved");
        let _ = &w.ctrl;
    });
}

/// Each plate keeps its slice (#547): back on a plate sliced before, with
/// nothing on it changed, its slice is held again without slicing.
#[test]
fn back_on_a_plate_its_slice_comes_back() {
    on_fresh_slint_thread(|| {
        let w = two_plates();
        w.document.set_active_plate(0);
        w.press_slice();
        w.document.set_active_plate(1);
        w.press_slice();
        assert_eq!(w.document.slice_params().map(|p| p.plate), Some(1));
        w.document.set_active_plate(0);
        assert_eq!(
            w.document.slice_params().map(|p| p.plate),
            Some(0),
            "plate 0's slice is back"
        );
        assert!(!w.document.held_slice_changed());
        // A Slice press now slices nothing.
        w.window.invoke_slice_requested();
        assert!(!w.window.get_slicing_in_progress(), "up to date");
        // Plate 1 back too, until something on it changes.
        w.document.set_active_plate(1);
        assert_eq!(w.document.slice_params().map(|p| p.plate), Some(1));
        w.document.set_active_plate(0);
        let edit =
            crate::plate_layout::moved_by(&w.document, w.cubes[1], glam::Vec3::new(3.0, 0.0, 0.0))
                .expect("a move");
        w.document.apply_edit(edit.clone()).expect("applies");
        w.document.push_edit(edit);
        w.document.set_active_plate(1);
        assert_eq!(
            w.document.slice_params().map(|p| p.plate),
            Some(0),
            "plate 1 changed: its slice is not brought back"
        );
    });
}
