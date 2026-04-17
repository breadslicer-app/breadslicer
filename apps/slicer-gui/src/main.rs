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

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use i18n::ctr;
use sc_mesh_core::scene::Scene;
use slint::ComponentHandle;

use about_window::wire_about_window;
use error_dialog::{format_anyhow_chain, show_error_dialog};
use mesh_render::Renderer;

slint::include_modules!();

const RENDER_WIDTH: u32 = 1024;
const RENDER_HEIGHT: u32 = 768;

type RendererCell = Rc<RefCell<Option<Renderer>>>;
type PendingSceneCell = Rc<RefCell<Option<Scene>>>;

fn main() -> Result<(), Box<dyn Error>> {
    slint::BackendSelector::new()
        .require_wgpu_28(slint::wgpu_28::WGPUConfiguration::default())
        .select()?;

    let ui = MainWindow::new()?;
    let renderer: RendererCell = Rc::new(RefCell::new(None));
    let pending_scene: PendingSceneCell = Rc::new(RefCell::new(None));

    install_rendering_notifier(&ui, renderer.clone(), pending_scene.clone())?;
    wire_close_scene(&ui, renderer, pending_scene.clone());
    wire_open_file(&ui, pending_scene);
    wire_about_window(&ui);

    ui.run()?;
    Ok(())
}

fn install_rendering_notifier(
    ui: &MainWindow,
    renderer: RendererCell,
    pending: PendingSceneCell,
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
                render_pending_scene(&renderer, &pending, &ui_weak);
            }
            _ => {}
        })
}

fn render_pending_scene(
    renderer: &RendererCell,
    pending: &PendingSceneCell,
    ui_weak: &slint::Weak<MainWindow>,
) {
    let Some(scene) = pending.borrow_mut().take() else {
        return;
    };
    let mut renderer_slot = renderer.borrow_mut();
    let Some(renderer) = renderer_slot.as_mut() else {
        return;
    };
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
    let Some(texture) = renderer.render(RENDER_WIDTH, RENDER_HEIGHT) else {
        return;
    };
    match slint::Image::try_from(texture) {
        Ok(image) => {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_mesh_rendering(image);
            }
        }
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

fn wire_open_file(ui: &MainWindow, pending: PendingSceneCell) {
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
        let ui_weak = ui_weak.clone();
        if let Err(err) = slint::spawn_local(async move {
            let Some(handle) = dialog.pick_file().await else {
                return;
            };
            let path = handle.path().to_path_buf();
            match mesh_loader::load_scene(&path) {
                Ok(scene) => {
                    *pending.borrow_mut() = Some(scene);
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

fn wire_close_scene(ui: &MainWindow, renderer: RendererCell, pending: PendingSceneCell) {
    let ui_weak = ui.as_weak();
    ui.on_close_current_scene(move || {
        *pending.borrow_mut() = None;
        if let Some(r) = renderer.borrow_mut().as_mut() {
            r.clear_scene();
        }
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_mesh_rendering(slint::Image::default());
        }
    });
}
