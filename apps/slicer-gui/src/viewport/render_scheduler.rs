/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Render-scheduling helpers: redraw requests, dirty marking, viewport
//! resize debounce, and the bucketed-size arithmetic the per-frame
//! render path uses while a resize is in flight.

use std::cell::Cell;

use slint::{ComponentHandle, Weak};

use crate::MainWindow;

use super::types::{FrameState, ViewportSize};

// Granularity for the allocation bucketing applied during active resize.
// A smaller value reduces the aspect distortion that `image-fit: fill` would
// apply while dragging (texture sized at bucket, stretched to logical); a
// larger value reduces reallocation rate. 64 px keeps the worst-case stretch
// under ~3% on a 1080p window, which is barely perceptible during a drag.
const RESIZE_BUCKET: u32 = 64;

/// How long `viewport-resized` must stay quiet before snapping back to
/// exact pixel sizing. Read by `wiring.rs` to set the debounce timer
/// duration; everything else flows through the helpers below.
pub(crate) const RESIZE_SETTLE: core::time::Duration = core::time::Duration::from_millis(200);

/// Pokes the Slint window so the next animation tick fires
/// `before-rendering`. No-op if the window has been dropped.
pub(super) fn request_redraw(ui: &Weak<MainWindow>) {
    if let Some(ui) = ui.upgrade() {
        ui.window().request_redraw();
    }
}

/// Marks the frame state as needing a redraw and pings the window.
/// Used by every state-mutation path that should result in a visible
/// update on the next render tick.
pub(super) fn mark_dirty(frame: &Cell<FrameState>, ui: &Weak<MainWindow>) {
    frame.update(FrameState::with_redraw_owed);
    request_redraw(ui);
}

/// `viewport-resized` callback handler. Stores the new size, latches
/// the resize-in-progress flag (so the next `handle_before_rendering`
/// runs the bucketed-size math), and pings the window.
pub(super) fn on_viewport_resized(
    viewport: &Cell<Option<ViewportSize>>,
    resizing: &Cell<bool>,
    frame: &Cell<FrameState>,
    ui: &Weak<MainWindow>,
    w: f32,
    h: f32,
) {
    viewport.set(ViewportSize::new(w, h));
    resizing.set(true);
    mark_dirty(frame, ui);
}

/// Called by the resize debounce timer once `viewport-resized` has
/// stayed quiet for `RESIZE_SETTLE`. Drops the bucketed allocation so
/// the next frame reallocates at the exact logical size.
pub(super) fn on_resize_settled(
    resizing: &Cell<bool>,
    frame: &Cell<FrameState>,
    ui: &Weak<MainWindow>,
    invalidate_renderer: impl FnOnce(),
) {
    resizing.set(false);
    invalidate_renderer();
    mark_dirty(frame, ui);
}

/// Effective render target size in physical pixels. While `resizing`
/// is true, rounds up to the next `RESIZE_BUCKET` multiple so drag
/// resizes don't churn the MSAA / target pool on every pointer move.
/// Once the debounce settles, `resizing` clears and the exact pixel
/// size returns.
pub(super) fn bucketed_size(width: u32, height: u32, resizing: bool) -> (u32, u32) {
    if resizing {
        (
            width.div_ceil(RESIZE_BUCKET) * RESIZE_BUCKET,
            height.div_ceil(RESIZE_BUCKET) * RESIZE_BUCKET,
        )
    } else {
        (width, height)
    }
}
