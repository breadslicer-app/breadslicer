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
mod error_dialog;
mod i18n;
mod mesh_loader;
mod mesh_render;

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;

use i18n::ctr;
use sc_mesh_core::scene::Scene;
use slint::ComponentHandle;

use crate::about_window::wire_about_window;
use crate::error_dialog::{format_anyhow_chain, show_error_dialog};
use crate::mesh_render::Renderer;

slint::include_modules!();

type RendererCell = Rc<RefCell<Option<Renderer>>>;
type PendingSceneCell = Rc<RefCell<Option<Scene>>>;
type ViewportCell = Rc<Cell<(f32, f32)>>;
type DirtyCell = Rc<Cell<bool>>;

fn main() -> Result<(), Box<dyn Error>> {
    slint::BackendSelector::new()
        .require_wgpu_28(slint::wgpu_28::WGPUConfiguration::default())
        .select()?;

    let ui = MainWindow::new()?;
    let renderer: RendererCell = Rc::new(RefCell::new(None));
    let pending_scene: PendingSceneCell = Rc::new(RefCell::new(None));
    let viewport: ViewportCell = Rc::new(Cell::new((0.0, 0.0)));
    let dirty: DirtyCell = Rc::new(Cell::new(false));

    install_rendering_notifier(
        &ui,
        renderer.clone(),
        pending_scene.clone(),
        viewport.clone(),
        dirty.clone(),
    )?;
    wire_close_scene(&ui, renderer, pending_scene.clone(), dirty.clone());
    wire_open_file(&ui, pending_scene, dirty.clone());
    wire_viewport_resized(&ui, viewport, dirty);
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

fn install_rendering_notifier(
    ui: &MainWindow,
    renderer: RendererCell,
    pending: PendingSceneCell,
    viewport: ViewportCell,
    dirty: DirtyCell,
) -> Result<(), slint::SetRenderingNotifierError> {
    let ui_weak = ui.as_weak();
    ui.window()
        .set_rendering_notifier(move |state, api| match state {
            slint::RenderingState::RenderingSetup => {
                let slint::GraphicsAPI::WGPU28 { device, queue, .. } = api else {
                    return;
                };
                *renderer.borrow_mut() = Some(Renderer::new(device.clone(), queue.clone()));
                if pending.borrow().is_some()
                    && let Some(ui) = ui_weak.upgrade()
                {
                    ui.window().request_redraw();
                }
            }
            slint::RenderingState::BeforeRendering => {
                render_frame(&renderer, &pending, &viewport, &dirty, &ui_weak);
            }
            _ => {}
        })
}

fn render_frame(
    renderer: &RendererCell,
    pending: &PendingSceneCell,
    viewport: &ViewportCell,
    dirty: &DirtyCell,
    ui_weak: &slint::Weak<MainWindow>,
) {
    let mut renderer_slot = renderer.borrow_mut();
    let Some(renderer) = renderer_slot.as_mut() else {
        return;
    };

    if let Some(scene) = pending.borrow_mut().take() {
        if let Err(err) = renderer.upload_scene(&scene) {
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
        dirty.set(true);
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

fn wire_open_file(ui: &MainWindow, pending: PendingSceneCell, dirty: DirtyCell) {
    let ui_weak = ui.as_weak();
    ui.on_open_file_requested(move || {
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

        let pending = pending.clone();
        let dirty = dirty.clone();
        let ui_weak = ui_weak.clone();
        if let Err(err) = slint::spawn_local(async move {
            let Some(handle) = dialog.pick_file().await else {
                return;
            };
            let path = handle.path().to_path_buf();
            match mesh_loader::load_scene(&path) {
                Ok(scene) => {
                    *pending.borrow_mut() = Some(scene);
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
        }) {
            eprintln!("Failed to spawn file dialog task: {err}");
        }
    });
}

fn wire_close_scene(
    ui: &MainWindow,
    renderer: RendererCell,
    pending: PendingSceneCell,
    dirty: DirtyCell,
) {
    let ui_weak = ui.as_weak();
    ui.on_close_current_scene(move || {
        *pending.borrow_mut() = None;
        dirty.set(false);
        if let Some(r) = renderer.borrow_mut().as_mut() {
            r.clear_scene();
        }
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_mesh_rendering(slint::Image::default());
        }
    });
}
