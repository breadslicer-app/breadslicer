/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod mesh_loader;
mod mesh_render;

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use sc_mesh_core::scene::Scene;
use slint::ComponentHandle;

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
        eprintln!("Failed to upload mesh to GPU: {err}");
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
        Err(err) => eprintln!("Failed to import wgpu texture into Slint: {err:?}"),
    }
}

fn wire_open_file(ui: &MainWindow, pending: PendingSceneCell) {
    let ui_weak = ui.as_weak();
    ui.on_open_file_requested(move || {
        let dialog = rfd::AsyncFileDialog::new()
            .add_filter("All files", &["obj", "stl", "stla", "stlb"])
            .add_filter("OBJ files", &["obj"])
            .add_filter("STL files", &["stl", "stla", "stlb"]);

        let pending = pending.clone();
        let ui_weak = ui_weak.clone();
        if let Err(err) = slint::spawn_local(async move {
            let Some(handle) = dialog.pick_file().await else {
                return;
            };
            match mesh_loader::load_scene(handle.path()) {
                Ok(scene) => {
                    *pending.borrow_mut() = Some(scene);
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.window().request_redraw();
                    }
                }
                Err(err) => eprintln!("Failed to load mesh '{}': {err}", handle.path().display()),
            }
        }) {
            eprintln!("Failed to spawn file dialog task: {err}");
        }
    });
}

fn wire_about_window(ui: &MainWindow) {
    let about_window: Rc<RefCell<Option<AboutWindow>>> = Rc::new(RefCell::new(None));
    ui.on_show_about_requested(move || {
        let mut slot = about_window.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            let _ = existing.show();
            return;
        }
        match build_about_window() {
            Ok(about) => {
                if let Err(err) = about.show() {
                    eprintln!("Failed to show About window: {err}");
                    return;
                }
                *slot = Some(about);
            }
            Err(err) => eprintln!("Failed to create About window: {err}"),
        }
    });
}

fn build_about_window() -> Result<AboutWindow, slint::PlatformError> {
    let about = AboutWindow::new()?;
    about.set_app_version(env!("CARGO_PKG_VERSION").into());

    let about_weak = about.as_weak();
    about.on_close_requested(move || {
        if let Some(w) = about_weak.upgrade() {
            let _ = w.hide();
        }
    });

    Ok(about)
}
