/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    draw_menus_in_window();
    sc_slicer_gui::run()
}

/// Has Slint draw its menus itself, in the window, on Windows (#149). As
/// native Win32 menus, which Slint destroys and makes again on every open,
/// the menu button's menu sometimes flashed white before taking the dark
/// theme. Menu shortcuts keep working: Slint handles them with or without a
/// native menu. macOS keeps its native menus.
#[cfg(windows)]
#[allow(unsafe_code)]
fn draw_menus_in_window() {
    // SAFETY: the first thing `main` does, before any thread exists, so no
    // other thread can read or write the environment at the same time.
    unsafe { std::env::set_var("SLINT_NO_MUDA", "1") };
}

#[cfg(not(windows))]
fn draw_menus_in_window() {}
