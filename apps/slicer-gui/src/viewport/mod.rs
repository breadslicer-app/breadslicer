/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

mod bed_clearance;
mod camera_controller;
mod controller;
mod frame_pacer;
mod gizmo_drag;
mod gizmo_frame_composer;
mod gizmo_fsm;
mod input_dispatch;
mod input_state;
mod keyboard_drive;
mod modal_transform;
pub(crate) mod numeric_transform;
mod object_list;
mod orbit_pivot;
mod performance_probe;
mod render_host;
mod render_scheduler;
mod session_state;
#[cfg(test)]
mod tests_controller_handle_before_rendering;
#[cfg(test)]
mod tests_controller_lifecycle;
#[cfg(test)]
mod tests_controller_numeric;
#[cfg(test)]
mod tests_controller_orbit;
#[cfg(test)]
mod tests_controller_section_gating;
#[cfg(test)]
mod tests_controller_supports_selection;
mod types;

pub(crate) use controller::ViewportController;
pub(crate) use object_list::build_object_list_names as object_names;
pub(crate) use performance_probe::{ProbeResult, ProbeUpdate};
pub(crate) use render_scheduler::RESIZE_SETTLE;
pub(crate) use types::{ScreenDelta, ScreenPoint};
