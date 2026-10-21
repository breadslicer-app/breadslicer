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

#[cfg(test)]
mod render_scheduler_tests {
    use super::*;

    // --- bucketed_size ---

    #[test]
    fn bucketed_size_returns_exact_when_not_resizing() {
        assert_eq!(bucketed_size(1234, 567, false), (1234, 567));
        assert_eq!(bucketed_size(0, 0, false), (0, 0));
    }

    #[test]
    fn bucketed_size_at_exact_multiple_is_idempotent_when_resizing() {
        assert_eq!(bucketed_size(64, 64, true), (64, 64));
        assert_eq!(bucketed_size(128, 256, true), (128, 256));
        assert_eq!(bucketed_size(1920, 1024, true), (1920, 1024));
    }

    #[test]
    fn bucketed_size_just_over_multiple_rounds_up_when_resizing() {
        // 65 rounds to 128, not 64; one-pixel-over crosses a bucket.
        assert_eq!(bucketed_size(65, 65, true), (128, 128));
    }

    #[test]
    fn bucketed_size_one_pixel_rounds_to_bucket_when_resizing() {
        // The smallest non-zero input must produce a full bucket so
        // the renderer always has a usable target.
        assert_eq!(bucketed_size(1, 1, true), (64, 64));
    }

    #[test]
    fn bucketed_size_zero_dimension_stays_zero_when_resizing() {
        // div_ceil(0, N) == 0, so a zero input stays zero. Callers
        // upstream must not feed the renderer a zero-area target;
        // this test pins that bucketing does not synthesise one.
        assert_eq!(bucketed_size(0, 0, true), (0, 0));
        assert_eq!(bucketed_size(0, 100, true), (0, 128));
        assert_eq!(bucketed_size(100, 0, true), (128, 0));
    }

    // --- mark_dirty / request_redraw ---
    //
    // The Slint side effects no-op against `Weak::default()`, so these
    // tests verify only the FrameState transitions and the no-panic
    // guarantee against a dropped window (matches the pattern used by
    // `ViewportController::new_for_test`).

    #[test]
    fn request_redraw_on_dropped_window_is_noop() {
        let ui: Weak<MainWindow> = Weak::default();
        request_redraw(&ui);
    }

    #[test]
    fn mark_dirty_sets_redraw_owed_from_clean() {
        let frame = Cell::new(FrameState::default());
        let ui: Weak<MainWindow> = Weak::default();
        mark_dirty(&frame, &ui);
        assert!(frame.get().redraw_owed());
        assert!(!frame.get().upload_owed());
    }

    #[test]
    fn mark_dirty_preserves_upload_owed() {
        let frame = Cell::new(FrameState::default().with_upload_owed());
        let ui: Weak<MainWindow> = Weak::default();
        mark_dirty(&frame, &ui);
        assert!(frame.get().redraw_owed());
        assert!(frame.get().upload_owed());
    }

    // --- on_viewport_resized ---

    #[test]
    fn on_viewport_resized_stores_size_flags_resizing_and_marks_dirty() {
        let viewport = Cell::new(None);
        let resizing = Cell::new(false);
        let frame = Cell::new(FrameState::default());
        let ui: Weak<MainWindow> = Weak::default();
        on_viewport_resized(&viewport, &resizing, &frame, &ui, 800.0, 600.0);

        let vp = viewport.get().expect("non-empty viewport size");
        assert_eq!(vp.width(), 800.0);
        assert_eq!(vp.height(), 600.0);
        assert!(resizing.get());
        assert!(frame.get().redraw_owed());
    }

    #[test]
    fn on_viewport_resized_with_invalid_size_clears_viewport() {
        // ViewportSize::new rejects zero / negative / non-finite
        // dimensions and returns None; the cell ends up empty.
        // resizing still latches so the next render uses the prior
        // bucket until a valid size arrives.
        let viewport = Cell::new(None);
        let resizing = Cell::new(false);
        let frame = Cell::new(FrameState::default());
        let ui: Weak<MainWindow> = Weak::default();
        on_viewport_resized(&viewport, &resizing, &frame, &ui, 0.0, 600.0);

        assert!(viewport.get().is_none());
        assert!(resizing.get());
        assert!(frame.get().redraw_owed());
    }

    // --- on_resize_settled ---

    #[test]
    fn on_resize_settled_clears_resizing_invokes_invalidator_and_marks_dirty() {
        let resizing = Cell::new(true);
        let frame = Cell::new(FrameState::default());
        let ui: Weak<MainWindow> = Weak::default();
        let invalidated = Cell::new(false);

        on_resize_settled(&resizing, &frame, &ui, || invalidated.set(true));

        assert!(!resizing.get());
        assert!(invalidated.get());
        assert!(frame.get().redraw_owed());
    }
}
