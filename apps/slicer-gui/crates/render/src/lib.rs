/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! The 3D view of the Breadslicer GUI, without the interface around it: the
//! wgpu renderer ([`mesh_render`]), the camera, picking, the transform gizmo,
//! the build volume, the slice cap, the front label, the print file previews
//! and the choice of graphics adapter. It knows nothing of the Slint windows,
//! so editing them does not rebuild it.

pub mod build_volume;
pub mod camera;
pub mod front_label;
pub mod gizmo;
mod gizmo_geometry;
pub mod graphics_adapter;
mod i18n;
pub mod mesh_render;
pub mod picking;
pub mod print_file_preview;
pub mod slice_cap;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests_print_file_preview;
pub mod under_plate;

use sc_slicer_gui_base::{breadcrumbs, startup_timing};
