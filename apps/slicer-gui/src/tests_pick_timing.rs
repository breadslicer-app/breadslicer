/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! How long the first click on a model just imported into an open scene
//! holds the window's thread (#395): the controller as the window drives it,
//! a box already on the plate, the heavy model imported next to it, then a
//! click on the model. Ignored: it needs the posed Wolverine STL (1.9 M
//! triangles).
//!
//! ```text
//! WOLVERINE_STL=<path> cargo test -p sc-slicer-gui --release --lib pick_timing -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::PointerModifiers;
use crate::mesh_render::{DeviceLoss, DeviceToken, Renderer};
use crate::test_support::gpu_devices::shared_test_device;
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::tests_model_import::{finish_loads, write_box};
use crate::viewport::{ScreenPoint, ViewportController};

const VIEW: (f32, f32) = (1280.0, 800.0);

/// The STL named by `WOLVERINE_STL`.
fn wolverine_path() -> Option<PathBuf> {
    std::env::var_os("WOLVERINE_STL").map(PathBuf::from)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// A controller with a real renderer when the machine has a GPU.
fn controller() -> Rc<ViewportController> {
    let ctrl = Rc::new(ViewportController::new_for_test());
    ctrl.register_document_listener();
    if let Some((device, queue)) = shared_test_device() {
        let token = DeviceToken::next();
        ctrl.loader().set_gpu(device.clone(), token, queue.clone());
        let renderer =
            Renderer::new(device, token, queue, DeviceLoss::default()).expect("a renderer");
        ctrl.set_renderer_for_test(Box::new(renderer));
    }
    ctrl
}

/// A press and a release at `at`, timed.
fn click(ctrl: &ViewportController, at: ScreenPoint) -> Duration {
    let started = Instant::now();
    ctrl.on_pointer_down(at, PointerModifiers::default());
    ctrl.on_pointer_up(at, PointerModifiers::default());
    started.elapsed()
}

#[test]
#[ignore = "needs the Wolverine STL named in WOLVERINE_STL"]
fn first_click_on_a_model_just_imported() {
    let Some(wolverine) = wolverine_path() else {
        eprintln!("WOLVERINE_STL names no file: skipped");
        return;
    };
    on_fresh_slint_thread(move || {
        let ctrl = controller();
        let dir = tempfile::tempdir().expect("tempdir");
        let base = write_box(dir.path(), "base.stl", [20.0, 20.0, 10.0]);
        ctrl.open_models(vec![base]);
        finish_loads(&ctrl);
        ctrl.handle_before_rendering();

        ctrl.open_models(vec![wolverine]);
        finish_loads(&ctrl);
        for _ in 0..3 {
            ctrl.handle_before_rendering();
        }
        let document = ctrl.document();
        let model = document
            .selection_primary_node_id()
            .expect("the import selects the model");

        ctrl.set_viewport_for_test(VIEW.0, VIEW.1);
        ctrl.frame_camera_for_test();
        let centre = document
            .with_scene(|spatial| {
                let query = spatial.leaf(model)?;
                let aabb = query.transformed_aabb(query.world_transform())?;
                let (lo, hi) = (aabb.min().coords(), aabb.max().coords());
                Some(std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5))
            })
            .flatten()
            .expect("the model has a box");
        let [x, y] = ctrl.project_for_test(centre).expect("the model is in view");
        let on_model = ScreenPoint::new(x, y);

        // A click on the sky misses every box: it clears the selection and
        // builds nothing.
        click(&ctrl, ScreenPoint::new(2.0, 2.0));
        assert_eq!(document.selection_primary_node_id(), None);

        let first = click(&ctrl, on_model);
        assert_eq!(
            document.selection_primary_node_id(),
            Some(model),
            "the click lands on the model"
        );
        click(&ctrl, ScreenPoint::new(2.0, 2.0));
        let second = click(&ctrl, on_model);
        eprintln!(
            "\n== first click on the imported model {:.2} ms; a later click {:.2} ms",
            ms(first),
            ms(second)
        );
    });
}
