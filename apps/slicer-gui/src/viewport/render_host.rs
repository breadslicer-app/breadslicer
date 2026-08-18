/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-viewport render-host concern. Owns the GPU [`Renderer`]
//! handle, the viewport size, the per-frame work flags
//! ([`FrameState`]), and the resize-debounce latch; exposes the
//! lifecycle callbacks (`handle_rendering_setup` / `handle_rendering_teardown`),
//! the resize callbacks (`on_viewport_resized` / `on_resize_settled`),
//! and the dirty-marking helpers consumed by the other concerns
//! through the coordinator.
//!
//! Holds a private [`slint::Weak`] clone of the main window so the
//! mark-dirty path (which pokes Slint's render loop in addition to
//! updating the local frame flag) is self-contained, the coordinator
//! does not have to thread the weak handle through every call.
//!
//! The per-frame braid in `handle_before_rendering` still lives on
//! the coordinator: this struct exposes the renderer cell and the
//! frame-state mutators it needs, but the cross-concern composition
//! (renderer + camera + document + input) is the coordinator's job.

use std::cell::{Cell, RefCell};

use slint::{ComponentHandle, Weak};

use super::render_scheduler;
use super::types::{FrameState, ViewportSize};
use crate::MainWindow;
use crate::mesh_render::{DeviceToken, MeshRenderer, Renderer, RendererInitError};

pub(super) struct RenderHost {
    ui: Weak<MainWindow>,
    renderer: RefCell<Option<Box<dyn MeshRenderer>>>,
    viewport: Cell<Option<ViewportSize>>,
    /// Per-frame work owed: a redraw (something visual changed) and/or
    /// a GPU upload (CPU-side scene differs from the renderer's
    /// buffers). Both can be set independently; a successful upload
    /// also sets the redraw flag so the just-uploaded scene is shown,
    /// and a redraw can be flagged alone (e.g. camera moved without a
    /// scene change).
    frame: Cell<FrameState>,
    /// True between a `viewport-resized` event and the debounce timer
    /// firing in `on_resize_settled`. While set,
    /// `handle_before_rendering` rounds the render target size up to
    /// the next `RESIZE_BUCKET` multiple so drag resizes don't churn
    /// the MSAA/target allocations on every pointer move.
    resizing: Cell<bool>,
}

impl RenderHost {
    pub(super) fn new(ui: &MainWindow) -> Self {
        Self {
            ui: ui.as_weak(),
            renderer: RefCell::new(None),
            viewport: Cell::new(None),
            frame: Cell::new(FrameState::default()),
            resizing: Cell::new(false),
        }
    }

    /// Constructor for the controller's `new_for_test`. The stored
    /// `Weak` is dangling, so every `ui.upgrade()` returns `None` and
    /// the Slint side-effects in mark_dirty / request_redraw silently
    /// skip, leaving the FrameState as the only observable signal.
    #[cfg(test)]
    pub(super) fn new_for_test() -> Self {
        Self {
            ui: Weak::default(),
            renderer: RefCell::new(None),
            viewport: Cell::new(None),
            frame: Cell::new(FrameState::default()),
            resizing: Cell::new(false),
        }
    }

    /// Borrow access to the renderer cell. Used by
    /// `handle_before_rendering` (which keeps the borrow live across
    /// upload, scene-bounds query, and render) and by the
    /// scene-install path.
    pub(super) fn renderer_cell(&self) -> &RefCell<Option<Box<dyn MeshRenderer>>> {
        &self.renderer
    }

    pub(super) fn viewport(&self) -> Option<ViewportSize> {
        self.viewport.get()
    }

    #[cfg(test)]
    pub(super) fn is_resizing(&self) -> bool {
        self.resizing.get()
    }

    pub(super) fn frame(&self) -> FrameState {
        self.frame.get()
    }

    pub(super) fn set_frame(&self, state: FrameState) {
        self.frame.set(state);
    }

    /// Composes a transition over the current [`FrameState`].
    /// Mirrors `Cell::update` for the underlying flag set.
    pub(super) fn update_frame<F>(&self, transition: F)
    where
        F: FnOnce(FrameState) -> FrameState,
    {
        self.frame.update(transition);
    }

    /// Marks the frame as needing a redraw and pings the Slint window
    /// so the next animation tick fires `before-rendering`.
    pub(super) fn mark_dirty(&self) {
        render_scheduler::mark_dirty(&self.frame, &self.ui);
    }

    pub(super) fn request_redraw(&self) {
        render_scheduler::request_redraw(&self.ui);
    }

    /// `viewport-resized` callback. Stores the new size, latches
    /// resize-in-progress, and pings the window.
    pub(super) fn on_viewport_resized(&self, w: f32, h: f32) {
        render_scheduler::on_viewport_resized(
            &self.viewport,
            &self.resizing,
            &self.frame,
            &self.ui,
            w,
            h,
        );
    }

    /// `viewport-resized` debounce-fired callback. Drops the bucketed
    /// MSAA / target allocations so the next frame reallocates at the
    /// exact logical size, eliminating the `image-fit: fill` stretch
    /// that persisted during the drag.
    pub(super) fn on_resize_settled(&self) {
        render_scheduler::on_resize_settled(&self.resizing, &self.frame, &self.ui, || {
            if let Some(r) = self.renderer.borrow_mut().as_mut() {
                r.invalidate_sizing();
            }
        });
    }

    /// Bucketed effective render-target size in physical pixels.
    /// Delegates to `render_scheduler::bucketed_size` against the
    /// live resize flag.
    pub(super) fn bucketed_size(&self, width: u32, height: u32) -> (u32, u32) {
        render_scheduler::bucketed_size(width, height, self.resizing.get())
    }

    pub(super) fn handle_rendering_setup(
        &self,
        device: slint::wgpu_29::wgpu::Device,
        device_token: DeviceToken,
        queue: slint::wgpu_29::wgpu::Queue,
    ) -> Result<(), RendererInitError> {
        let renderer = Renderer::new(device, device_token, queue)?;
        *self.renderer.borrow_mut() = Some(Box::new(renderer));
        Ok(())
    }

    /// `RenderingTeardown` callback handler. Drops the GPU renderer
    /// and clears the redraw-owed flag (nothing to render once the
    /// device is gone).
    pub(super) fn handle_rendering_teardown(&self) {
        *self.renderer.borrow_mut() = None;
        self.frame.update(FrameState::after_render);
    }

    /// Clears the renderer-side scene state without dropping the
    /// renderer itself. Called from the scene-cleared listener.
    pub(super) fn clear_renderer_scene(&self) {
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.clear_scene();
        }
    }

    // --- test-only accessors used by the coordinator's tests ------

    #[cfg(test)]
    pub(super) fn clear_frame_state(&self) {
        self.frame.set(FrameState::default());
    }

    #[cfg(test)]
    pub(super) fn redraw_owed(&self) -> bool {
        self.frame.get().redraw_owed()
    }

    #[cfg(test)]
    pub(super) fn set_viewport(&self, w: f32, h: f32) {
        self.viewport.set(ViewportSize::new(w, h));
    }
}

#[cfg(test)]
mod render_host_tests {
    use super::*;

    #[test]
    fn new_for_test_starts_clean() {
        let rh = RenderHost::new_for_test();
        assert!(rh.viewport().is_none());
        assert!(!rh.is_resizing());
        assert!(!rh.redraw_owed());
        assert!(rh.renderer_cell().borrow().is_none());
    }

    #[test]
    fn mark_dirty_sets_redraw_owed() {
        let rh = RenderHost::new_for_test();
        rh.mark_dirty();
        assert!(rh.redraw_owed());
    }

    #[test]
    fn clear_frame_state_drops_redraw_owed() {
        let rh = RenderHost::new_for_test();
        rh.mark_dirty();
        assert!(rh.redraw_owed());
        rh.clear_frame_state();
        assert!(!rh.redraw_owed());
    }

    #[test]
    fn on_viewport_resized_stores_size_and_latches_resizing() {
        let rh = RenderHost::new_for_test();
        rh.on_viewport_resized(800.0, 600.0);
        let vp = rh.viewport().expect("non-empty size");
        assert_eq!(vp.width(), 800.0);
        assert_eq!(vp.height(), 600.0);
        assert!(rh.is_resizing());
        assert!(rh.redraw_owed());
    }

    #[test]
    fn on_resize_settled_clears_resizing() {
        let rh = RenderHost::new_for_test();
        rh.on_viewport_resized(800.0, 600.0);
        rh.on_resize_settled();
        assert!(!rh.is_resizing());
    }

    #[test]
    fn bucketed_size_uses_live_resize_flag() {
        let rh = RenderHost::new_for_test();
        // Not resizing yet: exact pass-through.
        assert_eq!(rh.bucketed_size(1234, 567), (1234, 567));
        rh.on_viewport_resized(800.0, 600.0);
        // Now resizing: rounds up to the bucket.
        let (w, h) = rh.bucketed_size(65, 65);
        assert!(w >= 65);
        assert!(h >= 65);
    }

    #[test]
    fn handle_rendering_teardown_drops_renderer_slot_and_clears_redraw() {
        let rh = RenderHost::new_for_test();
        // Renderer slot starts None; teardown is a no-op on the slot
        // but still clears the redraw flag.
        rh.mark_dirty();
        assert!(rh.redraw_owed());
        rh.handle_rendering_teardown();
        assert!(rh.renderer_cell().borrow().is_none());
        assert!(!rh.redraw_owed());
    }
}
