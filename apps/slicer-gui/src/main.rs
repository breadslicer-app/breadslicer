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
mod app_settings;
mod app_state;
mod camera;
mod error_dialog;
mod gizmo;
mod gizmo_geometry;
mod i18n;
mod language;
mod mesh_loader;
mod mesh_render;
mod picking;
mod preferences_bus;
mod preferences_window;
mod scene_loader;
mod viewport;
mod window_prefs;
mod wiring;

use std::error::Error;

use crate::app_state::AppState;

slint::include_modules!();

fn main() -> Result<(), Box<dyn Error>> {
    slint::BackendSelector::new()
        .require_wgpu_28(slint::wgpu_28::WGPUConfiguration::default())
        .select()?;

    let app = AppState::new();
    app.spawn_main_window()?;
    slint::run_event_loop()?;
    Ok(())
}
