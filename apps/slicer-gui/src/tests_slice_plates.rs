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
use crate::hollow_run::HollowRun;
use crate::preferences_bus::PreferencesSnapshot;
use crate::support_run::SupportRun;
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::viewport::ViewportController;

/// The longest a slice may take before the test calls it hung.
const PATIENCE: Duration = Duration::from_secs(300);

struct Window {
    window: MainWindow,
    ctrl: Rc<ViewportController>,
    document: Rc<SceneDocument>,
    slice: Rc<SliceRun>,
    cost: Rc<crate::resin_cost::ResinCostPanel>,
    /// The layer timeline (#94).
    timeline: Rc<crate::layer_timeline::LayerTimeline>,
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
        default_resin: Some(MaterialId::new("siraya-tech-blu-easy-grey").expect("valid id")),
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
    let supports = SupportRun::install(
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
    let cost = crate::resin_cost::ResinCostPanel::install(
        &window,
        bus.clone(),
        document.clone(),
        ctrl.clone(),
    );
    let timeline =
        crate::layer_timeline::LayerTimeline::install(&window, ctrl.clone(), review.clone());
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
        cost,
        timeline,
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

/// A plate whose model sticks out of it is not sliced (tester,
/// 2026-11-18): the card names it and offers Arrange all; back on the
/// plate, it slices.
#[test]
fn a_model_past_the_plate_is_not_sliced() {
    on_fresh_slint_thread(|| {
        let w = two_plates();
        w.document.set_active_plate(0);
        let heading = "Models past the plate";
        let told = || {
            w.ctrl
                .dialogs()
                .error_headings_for_test()
                .iter()
                .filter(|h| h.as_str() == heading)
                .count()
        };
        let nudge = |dx: f32| {
            let edit = crate::plate_layout::moved_by(
                &w.document,
                w.cubes[0],
                glam::Vec3::new(dx, 0.0, 0.0),
            )
            .expect("a move");
            w.document.apply_edit(edit.clone()).expect("applies");
            w.document.push_edit(edit);
        };
        // The 10 mm cube 50 mm left of the centre, moved 58 mm more: 3.56 mm
        // past the plate's left edge.
        nudge(-58.0);
        w.window.invoke_slice_requested();
        assert!(!w.window.get_slicing_in_progress(), "nothing sliced");
        assert_eq!(told(), 1);
        nudge(58.0);
        w.press_slice();
        assert_eq!(told(), 1, "back on the plate, it slices");
    });
}

/// A plate whose model stands on a dead zone of the screen is not sliced
/// (#783): the card names it and offers Arrange all; with the zone off its
/// feet, it slices.
#[test]
fn a_model_on_a_dead_zone_is_not_sliced() {
    use crate::dead_zones::DeadZone;
    on_fresh_slint_thread(|| {
        let w = two_plates();
        w.document.set_active_plate(0);
        let heading = "Models on a dead zone";
        let told = || {
            w.ctrl
                .dialogs()
                .error_headings_for_test()
                .iter()
                .filter(|h| h.as_str() == heading)
                .count()
        };
        // The 10 mm cube stands 50 mm left of the first plate's centre: a
        // cell under its corner.
        w.ctrl
            .set_dead_zones(vec![DeadZone::between([-46.0, 3.0], [-41.0, 8.0])], 2);
        w.window.invoke_slice_requested();
        assert!(!w.window.get_slicing_in_progress(), "nothing sliced");
        assert_eq!(told(), 1);
        // A zone beside it, not under it.
        w.ctrl
            .set_dead_zones(vec![DeadZone::between([-40.0, 3.0], [-35.0, 8.0])], 2);
        w.press_slice();
        assert_eq!(told(), 1, "off the zone, it slices");
    });
}

/// With `BREADSLICER_UI_SHOTS` naming a folder, the Slice tab of a short
/// window after a slice (#628): the tab scrolls as a whole,
/// so the review keeps its room; scrolled to the top and to the bottom.
#[test]
fn the_slice_tab_of_a_short_window_is_pictured_when_asked() {
    let Some(dir) = std::env::var_os("BREADSLICER_UI_SHOTS").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    on_fresh_slint_thread(move || {
        let w = two_plates();
        w.window.show().expect("the window shows");
        w.window
            .window()
            .set_size(slint::LogicalSize::new(1280.0, 720.0));
        w.window.set_has_scene(true);
        w.ctrl.register_document_listener();
        w.document.set_active_plate(0);
        w.press_slice();
        w.window.invoke_slice_price_edited(0, "30".into());
        w.window.invoke_slice_price_edited(1, "$".into());
        w.window.invoke_slice_price_edited(2, "1000".into());
        let shot = |name: &str| {
            crate::test_support::slint_thread::render_frames();
            let picture = w.window.window().take_snapshot().expect("a snapshot");
            image::RgbaImage::from_raw(
                picture.width(),
                picture.height(),
                picture.as_bytes().to_vec(),
            )
            .expect("RGBA")
            .save(dir.join(format!("slice-short-{name}.png")))
            .expect("written");
        };
        shot("top");
        w.window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(1150.0, 400.0),
                delta_x: 0.0,
                delta_y: -2000.0,
            });
        shot("bottom");
    });
}

/// The resin's price makes the cost of the print (#101): the resin it
/// cures at the bottle's price per ml, with the losses on top; weighing what
/// the bottle lost sets the losses.
#[test]
fn a_priced_resin_tells_what_the_print_costs() {
    on_fresh_slint_thread(|| {
        let w = two_plates();
        // The estimate is worked out as the window's controller hears of the
        // slice.
        w.ctrl.register_document_listener();
        w.document.set_active_plate(1);
        w.press_slice();
        let estimate = w.ctrl.last_estimate().unwrap_or_else(|| {
            panic!(
                "an estimate: {} ({})",
                w.window.get_slice_estimate_note(),
                w.window.get_slice_estimate_valid()
            )
        });
        assert_eq!(w.window.get_slice_cost(), "", "no price yet");
        assert!(!w.window.get_slice_cost_note().is_empty());
        w.window.invoke_slice_price_edited(1, "$".into());
        w.window.invoke_slice_price_edited(0, "30".into());
        w.window.invoke_slice_price_edited(2, "1000".into());
        let cost = estimate.volume_ml * 30.0 / 1000.0;
        assert_eq!(
            w.window.get_slice_cost().as_str(),
            crate::resin_cost::money("$", cost)
        );
        // Weighed: 10 % more than the slice cures.
        let liquid = estimate.liquid_g.expect("the resin gives a density");
        w.window
            .invoke_slice_price_edited(4, format!("{:.4}", liquid * 1.1).into());
        assert_eq!(w.window.get_slice_price_losses().as_str(), "10.0");
        assert_eq!(
            w.window.get_slice_cost().as_str(),
            crate::resin_cost::money("$", cost * 1.1)
        );
        // Kept for the resin, and an emptied price takes it away.
        let snapshot = w.cost_bus_snapshot();
        assert_eq!(snapshot.len(), 1);
        w.window.invoke_slice_price_edited(0, "".into());
        assert_eq!(w.window.get_slice_cost(), "");
        assert!(w.cost_bus_snapshot().is_empty());
    });
}

/// The cost card's fields keep what was typed when the tab changes and the
/// card is drawn again (#747), and the currency is picked from a list.
#[test]
fn the_cost_fields_keep_what_was_typed() {
    on_fresh_slint_thread(|| {
        let w = two_plates();
        w.ctrl.register_document_listener();
        w.document.set_active_plate(1);
        w.press_slice();
        let estimate = w.ctrl.last_estimate().expect("an estimate");
        // A price typed before the bottle's content: not a price yet, but the
        // field keeps it.
        w.window.invoke_slice_price_edited(0, "30".into());
        assert_eq!(w.window.get_slice_price_text().as_str(), "30");
        use slint::Model as _;
        let labels: Vec<String> = w
            .window
            .get_slice_price_currencies()
            .iter()
            .map(|label| label.to_string())
            .collect();
        let euro = labels
            .iter()
            .find(|label| label.starts_with("EUR"))
            .expect("the euro on the list")
            .clone();
        w.window.invoke_slice_price_edited(1, euro.as_str().into());
        assert_eq!(w.window.get_slice_price_currency().as_str(), "\u{20ac}");
        let at = w.window.get_slice_price_currency_index();
        assert_eq!(labels[at as usize], euro, "the euro shows picked");
        w.window.invoke_slice_price_edited(2, "1000".into());
        w.window.invoke_slice_price_edited(3, "5".into());
        assert_eq!(w.window.get_slice_price_amount().as_str(), "1000");
        assert_eq!(w.window.get_slice_price_losses().as_str(), "5");
        let liquid = estimate.liquid_g.expect("the resin gives a density");
        let weighed = format!("{:.4}", liquid * 1.2);
        w.window
            .invoke_slice_price_edited(4, weighed.as_str().into());
        assert_eq!(w.window.get_slice_price_weighed().as_str(), weighed);
        assert_eq!(w.window.get_slice_price_losses().as_str(), "20.0");
        let cost = estimate.volume_ml * 30.0 / 1000.0 * 1.2;
        assert_eq!(
            w.window.get_slice_cost().as_str(),
            crate::resin_cost::money("\u{20ac}", cost)
        );
        assert_eq!(
            w.cost_bus_snapshot().values().next().unwrap().currency,
            "\u{20ac}"
        );
    });
}

/// Slice pressed while supports are being made waits for them and then
/// slices, saying so (#750); before, the press did nothing.
#[test]
fn a_slice_pressed_while_supports_are_made_waits_for_them() {
    on_fresh_slint_thread(|| {
        let w = two_plates();
        w.window.set_support_gen_in_progress(true);
        w.window.invoke_slice_requested();
        assert!(!w.window.get_slicing_in_progress(), "it waits");
        assert!(!w.window.get_status_notice().is_empty(), "and says so");
        // Still making them: the next look keeps waiting.
        w.slice.forces_tick();
        assert!(!w.window.get_slicing_in_progress());
        w.window.set_support_gen_in_progress(false);
        w.slice.forces_tick();
        assert!(w.window.get_status_notice().is_empty());
        w.finish();
        assert!(w.document.slice_params().is_some(), "it sliced");
    });
}

impl Window {
    fn cost_bus_snapshot(
        &self,
    ) -> std::collections::BTreeMap<MaterialId, crate::resin_cost::ResinPrice> {
        self.cost.prices_for_test()
    }
}

/// The layer timeline follows the slice held (#94): one row per layer, the
/// force a cube's section pulls with, and a readout of the layer on show.
#[test]
fn the_layer_timeline_shows_each_layer_of_the_slice() {
    on_fresh_slint_thread(|| {
        let w = two_plates();
        assert!(!w.window.get_slice_timeline_valid());
        w.document.set_active_plate(0);
        w.press_slice();
        let count = w.document.slice_layer_count().expect("the slice landed") as usize;
        let shown = w.timeline.shown_for_test();
        assert_eq!(shown.layers.len(), count);
        assert!(w.window.get_slice_timeline_valid());
        assert!(!w.window.get_slice_timeline_force().is_empty());
        // The 10 mm cube: 100 mm² and 40 mm of edge on a middle layer,
        // about 1.5 N on FEP.
        let middle = shown.layers[count / 2];
        assert!((middle.area_mm2 - 100.0).abs() < 2.0, "{middle:?}");
        assert!((middle.perimeter_mm - 40.0).abs() < 1.5, "{middle:?}");
        assert!((middle.force_n - 1.5).abs() < 0.1, "{middle:?}");
        assert!(middle.time_sec > 0.0);
        let total: f64 = shown.layers.iter().map(|layer| layer.time_sec).sum();
        assert!(total > 0.0);
        let line = w
            .window
            .invoke_slice_timeline_readout((count / 2) as i32, 0)
            .to_string();
        assert!(line.contains(" N"), "{line}");
    });
}
