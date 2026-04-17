/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod about_window;
mod camera;
mod error_dialog;
mod i18n;
mod mesh_loader;
mod mesh_render;

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use i18n::ctr;
use sc_mesh_core::scene::Scene;
use slint::ComponentHandle;

use crate::about_window::wire_about_window;
use crate::camera::{AxisView, OrbitCamera};
use crate::error_dialog::{format_anyhow_chain, show_error_dialog};
use crate::mesh_render::Renderer;

slint::include_modules!();

type RendererCell = Rc<RefCell<Option<Renderer>>>;
type SceneCell = Rc<RefCell<Option<Scene>>>;
type CameraCell = Rc<RefCell<Option<OrbitCamera>>>;
type ViewportCell = Rc<Cell<(f32, f32)>>;
type DirtyCell = Rc<Cell<bool>>;
type NeedsUploadCell = Rc<Cell<bool>>;
type LoadGenCell = Rc<Cell<u64>>;
type LoadSlot = Arc<Mutex<Option<(u64, PathBuf, anyhow::Result<Scene>)>>>;

fn main() -> Result<(), Box<dyn Error>> {
    slint::BackendSelector::new()
        .require_wgpu_28(slint::wgpu_28::WGPUConfiguration::default())
        .select()?;

    let ui = MainWindow::new()?;
    let renderer: RendererCell = Rc::new(RefCell::new(None));
    let scene: SceneCell = Rc::new(RefCell::new(None));
    let saved_camera: CameraCell = Rc::new(RefCell::new(None));
    let viewport: ViewportCell = Rc::new(Cell::new((0.0, 0.0)));
    let dirty: DirtyCell = Rc::new(Cell::new(false));
    let needs_upload: NeedsUploadCell = Rc::new(Cell::new(false));
    let load_gen: LoadGenCell = Rc::new(Cell::new(0));

    install_rendering_notifier(
        &ui,
        renderer.clone(),
        scene.clone(),
        saved_camera.clone(),
        viewport.clone(),
        dirty.clone(),
        needs_upload.clone(),
    )?;
    wire_close_scene(
        &ui,
        renderer.clone(),
        scene.clone(),
        saved_camera.clone(),
        dirty.clone(),
        needs_upload.clone(),
        load_gen.clone(),
    );
    wire_open_file(
        &ui,
        scene,
        saved_camera,
        dirty.clone(),
        needs_upload,
        load_gen,
    );
    wire_viewport_resized(&ui, viewport.clone(), dirty.clone());
    wire_viewport_input(&ui, renderer, viewport, dirty);
    wire_about_window(&ui);

    ui.run()?;
    Ok(())
}

fn wire_viewport_resized(ui: &MainWindow, viewport: ViewportCell, dirty: DirtyCell) {
    let ui_weak = ui.as_weak();
    ui.on_viewport_resized(move |w, h| {
        viewport.set((w, h));
        dirty.set(true);
        if let Some(ui) = ui_weak.upgrade() {
            ui.window().request_redraw();
        }
    });
}

fn wire_viewport_input(
    ui: &MainWindow,
    renderer: RendererCell,
    viewport: ViewportCell,
    dirty: DirtyCell,
) {
    let ui_weak = ui.as_weak();
    let r = renderer.clone();
    let v = viewport.clone();
    let d = dirty.clone();
    let uw = ui_weak.clone();
    ui.on_rotate_drag(move |dx, dy| {
        let (_, h) = v.get();
        if let Some(r) = r.borrow_mut().as_mut()
            && r.rotate(dx, dy, h)
        {
            d.set(true);
            if let Some(ui) = uw.upgrade() {
                ui.window().request_redraw();
            }
        }
    });

    let r = renderer.clone();
    let v = viewport.clone();
    let d = dirty.clone();
    let uw = ui_weak.clone();
    ui.on_pan_drag(move |dx, dy| {
        let (_, h) = v.get();
        if let Some(r) = r.borrow_mut().as_mut()
            && r.pan(dx, dy, h)
        {
            d.set(true);
            if let Some(ui) = uw.upgrade() {
                ui.window().request_redraw();
            }
        }
    });

    let r = renderer.clone();
    let v = viewport.clone();
    let d = dirty.clone();
    let uw = ui_weak.clone();
    ui.on_zoom_at(move |delta, cursor_x, cursor_y| {
        let (w, h) = v.get();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let factor = (-delta * 0.005).exp().clamp(0.1, 10.0);
        if let Some(r) = r.borrow_mut().as_mut()
            && r.zoom_at(factor, cursor_x, cursor_y, w, h)
        {
            d.set(true);
            if let Some(ui) = uw.upgrade() {
                ui.window().request_redraw();
            }
        }
    });

    let r = renderer.clone();
    let d = dirty.clone();
    let uw = ui_weak.clone();
    ui.on_frame_view_requested(move || {
        if let Some(r) = r.borrow_mut().as_mut()
            && r.frame_view()
        {
            d.set(true);
            if let Some(ui) = uw.upgrade() {
                ui.window().request_redraw();
            }
        }
    });

    ui.on_axis_view_requested(move |view| {
        let Some(axis) = axis_from_code(view) else {
            return;
        };
        if let Some(r) = renderer.borrow_mut().as_mut()
            && r.set_axis_view(axis)
        {
            dirty.set(true);
            if let Some(ui) = ui_weak.upgrade() {
                ui.window().request_redraw();
            }
        }
    });
}

fn axis_from_code(code: i32) -> Option<AxisView> {
    match code {
        0 => Some(AxisView::Front),
        1 => Some(AxisView::Back),
        2 => Some(AxisView::Right),
        3 => Some(AxisView::Left),
        4 => Some(AxisView::Top),
        5 => Some(AxisView::Bottom),
        _ => None,
    }
}

fn install_rendering_notifier(
    ui: &MainWindow,
    renderer: RendererCell,
    scene: SceneCell,
    saved_camera: CameraCell,
    viewport: ViewportCell,
    dirty: DirtyCell,
    needs_upload: NeedsUploadCell,
) -> Result<(), slint::SetRenderingNotifierError> {
    let ui_weak = ui.as_weak();
    ui.window()
        .set_rendering_notifier(move |state, api| match state {
            slint::RenderingState::RenderingSetup => {
                let slint::GraphicsAPI::WGPU28 { device, queue, .. } = api else {
                    return;
                };
                *renderer.borrow_mut() = Some(Renderer::new(device.clone(), queue.clone()));
                if scene.borrow().is_some() {
                    needs_upload.set(true);
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.window().request_redraw();
                    }
                }
            }
            slint::RenderingState::BeforeRendering => {
                render_frame(
                    &renderer,
                    &scene,
                    &saved_camera,
                    &needs_upload,
                    &viewport,
                    &dirty,
                    &ui_weak,
                );
            }
            // Release GPU-owning state before the graphics context goes away.
            // The Scene (CPU-side) and a camera snapshot are kept so the next
            // RenderingSetup can re-upload and restore the user's view.
            slint::RenderingState::RenderingTeardown => {
                let camera_snapshot = renderer.borrow().as_ref().and_then(|r| r.camera());
                if camera_snapshot.is_some() {
                    *saved_camera.borrow_mut() = camera_snapshot;
                }
                *renderer.borrow_mut() = None;
                dirty.set(false);
                if scene.borrow().is_some() {
                    needs_upload.set(true);
                }
            }
            _ => {}
        })
}

fn render_frame(
    renderer: &RendererCell,
    scene: &SceneCell,
    saved_camera: &CameraCell,
    needs_upload: &NeedsUploadCell,
    viewport: &ViewportCell,
    dirty: &DirtyCell,
    ui_weak: &slint::Weak<MainWindow>,
) {
    let mut renderer_slot = renderer.borrow_mut();
    let Some(renderer) = renderer_slot.as_mut() else {
        return;
    };

    if needs_upload.get() {
        needs_upload.set(false);
        if let Some(scene_ref) = scene.borrow().as_ref() {
            if let Err(err) = renderer.upload_scene(scene_ref) {
                show_error_dialog(
                    ctr!("ErrorDialog", "Failed to load the 3D model").as_str(),
                    Some(
                        ctr!(
                            "ErrorDialog",
                            "The mesh could not be prepared for rendering."
                        )
                        .to_string(),
                    ),
                    err.to_string(),
                    format_anyhow_chain(&err),
                );
                return;
            }
            if let Some(cam) = saved_camera.borrow_mut().take() {
                renderer.set_camera(cam);
            }
            dirty.set(true);
        }
    }

    if !dirty.get() {
        return;
    }

    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    let (w_logical, h_logical) = viewport.get();
    let scale = ui.window().scale_factor();
    let width = (w_logical * scale).round() as u32;
    let height = (h_logical * scale).round() as u32;
    if width == 0 || height == 0 {
        return;
    }

    let Some(texture) = renderer.render(width, height) else {
        return;
    };
    dirty.set(false);
    match slint::Image::try_from(texture) {
        Ok(image) => ui.set_mesh_rendering(image),
        Err(err) => show_error_dialog(
            ctr!("ErrorDialog", "Failed to display the 3D model").as_str(),
            Some(
                ctr!(
                    "ErrorDialog",
                    "The rendered texture could not be handed to the UI."
                )
                .to_string(),
            ),
            err.to_string(),
            format!("{err:?}"),
        ),
    }
}

fn wire_open_file(
    ui: &MainWindow,
    scene: SceneCell,
    saved_camera: CameraCell,
    dirty: DirtyCell,
    needs_upload: NeedsUploadCell,
    load_gen: LoadGenCell,
) {
    // Shared mailbox between the parsing worker and the UI-thread consumer.
    let slot: LoadSlot = Arc::new(Mutex::new(None));

    // Runs on the UI thread when a background load finishes. Drains the slot,
    // checks the generation against the current one (to discard stale results
    // from loads the user has since cancelled via Close), and deposits.
    {
        let slot = slot.clone();
        let load_gen = load_gen.clone();
        let ui_weak = ui.as_weak();
        ui.on_scene_load_finished(move || {
            let taken = slot.lock().expect("load slot poisoned").take();
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_loading(false);
            }
            let Some((stamp, path, result)) = taken else {
                return;
            };
            if stamp != load_gen.get() {
                // The user cancelled (Close) or something else bumped the
                // generation while this worker was parsing. Drop the result.
                return;
            }
            match result {
                Ok(loaded) => {
                    *scene.borrow_mut() = Some(loaded);
                    *saved_camera.borrow_mut() = None;
                    needs_upload.set(true);
                    dirty.set(true);
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.window().request_redraw();
                    }
                }
                Err(err) => show_error_dialog(
                    ctr!("ErrorDialog", "Failed to open 3D model").as_str(),
                    Some(ctr!("ErrorDialog", "File: {}", path.display()).to_string()),
                    err.to_string(),
                    format_anyhow_chain(&err),
                ),
            }
        });
    }

    let ui_weak = ui.as_weak();
    ui.on_open_file_requested(move || {
        // The Slint event loop keeps processing callbacks while the native
        // file dialog is open, and `enabled: !root.loading` on the menu item
        // doesn't block that. So gate here in Rust *before* touching the
        // dialog, and flip `loading` immediately (not after pick_file awaits).
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if ui.get_loading() {
            return;
        }
        ui.set_loading(true);

        let dialog = rfd::AsyncFileDialog::new()
            .add_filter(
                ctr!("FileDialog", "All files").to_string(),
                &["obj", "stl", "stla", "stlb"],
            )
            .add_filter(ctr!("FileDialog", "OBJ files").to_string(), &["obj"])
            .add_filter(
                ctr!("FileDialog", "STL files").to_string(),
                &["stl", "stla", "stlb"],
            );

        let slot = slot.clone();
        let load_gen = load_gen.clone();
        let ui_weak_inner = ui_weak.clone();
        let ui_weak_for_err = ui_weak.clone();
        if let Err(err) = slint::spawn_local(async move {
            let Some(handle) = dialog.pick_file().await else {
                // User cancelled the dialog: clear the gate.
                if let Some(ui) = ui_weak_inner.upgrade() {
                    ui.set_loading(false);
                }
                return;
            };
            let path = handle.path().to_path_buf();

            let my_stamp = load_gen.get().wrapping_add(1);
            load_gen.set(my_stamp);

            let worker_ui_weak = ui_weak_inner.clone();
            std::thread::spawn(move || {
                // Catch a panic in the parser so the UI always gets notified
                // and the loading flag is cleared.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    mesh_loader::load_scene(&path)
                }))
                .unwrap_or_else(|payload| {
                    let msg = panic_message(&payload);
                    Err(anyhow::anyhow!("The mesh loader panicked: {msg}"))
                });

                *slot.lock().expect("load slot poisoned") = Some((my_stamp, path, result));
                let _ = worker_ui_weak.upgrade_in_event_loop(|ui| {
                    ui.invoke_scene_load_finished();
                });
            });
        }) {
            eprintln!("Failed to spawn file dialog task: {err}");
            if let Some(ui) = ui_weak_for_err.upgrade() {
                ui.set_loading(false);
            }
        }
    });
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn wire_close_scene(
    ui: &MainWindow,
    renderer: RendererCell,
    scene: SceneCell,
    saved_camera: CameraCell,
    dirty: DirtyCell,
    needs_upload: NeedsUploadCell,
    load_gen: LoadGenCell,
) {
    let ui_weak = ui.as_weak();
    ui.on_close_current_scene(move || {
        // Invalidate any in-flight load so its result is dropped on arrival.
        load_gen.set(load_gen.get().wrapping_add(1));
        *scene.borrow_mut() = None;
        *saved_camera.borrow_mut() = None;
        needs_upload.set(false);
        dirty.set(false);
        if let Some(r) = renderer.borrow_mut().as_mut() {
            r.clear_scene();
        }
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_mesh_rendering(slint::Image::default());
        }
    });
}
