/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Runtime window icon for the taskbar and switcher.
//!
//! The executable already carries the icon as a resource (see the GUI
//! build script), which Windows uses for the file, shortcuts, and the
//! title bar. The taskbar button of a live window is a separate, runtime
//! property that the windowing layer does not populate from that
//! resource, so it is set here from the same committed `.ico`. winit
//! applies the icon on the platforms that support it (Windows, X11) and
//! ignores it elsewhere.

use std::io::Cursor;

use slint::winit_030::{WinitWindowAccessor, winit::window::Icon};

/// The multi-resolution icon, embedded into the binary. Shares the file
/// the executable resource is built from, so the live window and the
/// file as Explorer shows it never drift apart.
const ICON_BYTES: &[u8] = include_bytes!("../assets/icons/breadslicer.ico");

/// Applies the application icon to a Slint window so it shows in the
/// taskbar and the window switcher. A best-effort step: if the window
/// is not backed by winit, or the icon cannot be decoded, the window
/// simply keeps the platform default.
pub(crate) fn apply(window: &slint::Window) {
    let Some(icon) = decode_icon() else {
        return;
    };
    window.with_winit_window(|winit_window| {
        winit_window.set_window_icon(Some(icon.clone()));
    });
}

/// Decodes the largest frame of the embedded icon into a winit [`Icon`].
/// The largest frame is chosen so the system has the most pixels to
/// downscale from for every taskbar size and DPI. Returns `None` if the
/// bytes cannot be parsed (they are committed and validated, so this is
/// not expected in practice).
fn decode_icon() -> Option<Icon> {
    let dir = ico::IconDir::read(Cursor::new(ICON_BYTES)).ok()?;
    let entry = dir
        .entries()
        .iter()
        .max_by_key(|entry| entry.width() as u64 * entry.height() as u64)?;
    let image = entry.decode().ok()?;
    let (width, height) = (image.width(), image.height());
    Icon::from_rgba(image.rgba_data().to_vec(), width, height).ok()
}
