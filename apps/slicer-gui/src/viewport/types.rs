/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

/// A logical-pixel position in the viewport's coordinate system (Slint
/// hands these to the controller as raw `f32` pairs). The constructor
/// keeps the type opaque at the public callback boundary so a future
/// validation step (NaN rejection, scale-factor application) has a
/// single seam to land on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScreenPoint {
    pub(super) x: f32,
    pub(super) y: f32,
}

impl ScreenPoint {
    pub(crate) fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A logical-pixel drag delta in the viewport's coordinate system: how
/// far the pointer moved between two events, not where it sits. Kept
/// distinct from [`ScreenPoint`] so the type system rejects e.g.
/// passing a cursor position where a drag delta is expected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScreenDelta {
    pub(super) dx: f32,
    pub(super) dy: f32,
}

impl ScreenDelta {
    pub(crate) fn new(dx: f32, dy: f32) -> Self {
        Self { dx, dy }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum InputPhase {
    Idle,
    Pressed { origin: ScreenPoint },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct FrameState {
    redraw_owed: bool,
    upload_owed: bool,
}

impl FrameState {
    pub(super) fn redraw_owed(self) -> bool {
        self.redraw_owed
    }

    pub(super) fn upload_owed(self) -> bool {
        self.upload_owed
    }

    pub(super) fn with_redraw_owed(mut self) -> Self {
        self.redraw_owed = true;
        self
    }

    pub(super) fn with_upload_owed(mut self) -> Self {
        self.upload_owed = true;
        self
    }

    pub(super) fn after_render(mut self) -> Self {
        self.redraw_owed = false;
        self
    }

    pub(super) fn after_upload_success(mut self) -> Self {
        self.upload_owed = false;
        self.redraw_owed = true;
        self
    }

    pub(super) fn upload_cleared(mut self) -> Self {
        self.upload_owed = false;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ViewportSize {
    w: f32,
    h: f32,
}

impl ViewportSize {
    pub(super) fn new(w: f32, h: f32) -> Option<Self> {
        if w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite() {
            Some(Self { w, h })
        } else {
            None
        }
    }

    pub(super) fn width(self) -> f32 {
        self.w
    }

    pub(super) fn height(self) -> f32 {
        self.h
    }
}

#[cfg(test)]
mod viewport_size_tests {
    use super::*;

    #[test]
    fn finite_positive_pair_is_accepted() {
        let vp = ViewportSize::new(1920.0, 1080.0).expect("valid size");
        assert_eq!(vp.width(), 1920.0);
        assert_eq!(vp.height(), 1080.0);
    }

    #[test]
    fn zero_or_negative_dimensions_are_rejected() {
        assert!(ViewportSize::new(0.0, 100.0).is_none());
        assert!(ViewportSize::new(100.0, 0.0).is_none());
        assert!(ViewportSize::new(-1.0, 100.0).is_none());
        assert!(ViewportSize::new(100.0, -1.0).is_none());
    }

    #[test]
    fn nan_or_infinite_dimensions_are_rejected() {
        assert!(ViewportSize::new(f32::NAN, 100.0).is_none());
        assert!(ViewportSize::new(100.0, f32::NAN).is_none());
        assert!(ViewportSize::new(f32::INFINITY, 100.0).is_none());
        assert!(ViewportSize::new(100.0, f32::INFINITY).is_none());
        assert!(ViewportSize::new(f32::NEG_INFINITY, 100.0).is_none());
    }
}

#[cfg(test)]
mod frame_state_tests {
    use super::*;

    #[test]
    fn default_is_clean() {
        let s = FrameState::default();
        assert!(!s.redraw_owed());
        assert!(!s.upload_owed());
    }

    #[test]
    fn with_redraw_owed_sets_only_redraw() {
        let s = FrameState::default().with_redraw_owed();
        assert!(s.redraw_owed());
        assert!(!s.upload_owed());
    }

    #[test]
    fn with_upload_owed_sets_only_upload() {
        let s = FrameState::default().with_upload_owed();
        assert!(!s.redraw_owed());
        assert!(s.upload_owed());
    }

    #[test]
    fn both_flags_can_coexist() {
        let s = FrameState::default().with_redraw_owed().with_upload_owed();
        assert!(s.redraw_owed());
        assert!(s.upload_owed());
    }

    #[test]
    fn after_render_clears_redraw_only() {
        let s = FrameState::default()
            .with_redraw_owed()
            .with_upload_owed()
            .after_render();
        assert!(!s.redraw_owed());
        assert!(s.upload_owed());
    }

    #[test]
    fn after_upload_success_clears_upload_and_sets_redraw() {
        let s = FrameState::default()
            .with_upload_owed()
            .after_upload_success();
        assert!(s.redraw_owed());
        assert!(!s.upload_owed());
    }

    #[test]
    fn upload_cleared_clears_upload_only() {
        let s = FrameState::default()
            .with_redraw_owed()
            .with_upload_owed()
            .upload_cleared();
        assert!(s.redraw_owed());
        assert!(!s.upload_owed());
    }
}
