/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Press / release input phase tracking, plus the click-vs-drag
//! threshold used by `on_pointer_up` to decide whether to run the
//! picker.

use std::cell::Cell;

use super::types::{InputPhase, ScreenPoint};

/// Maximum logical-pixel distance between a left-button press and its
/// release for the gesture to count as a click rather than a drag.
/// 4 px is roughly two-finger-on-trackpad jitter; tighter than this and
/// users with imprecise pointers fail to select; looser and stationary
/// clicks bleed into starts-of-drags.
pub(super) const CLICK_DRAG_THRESHOLD: f32 = 4.0;

/// Outcome of a press → release sequence as classified by
/// [`InputState::click_or_drag`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ClickOrDrag {
    /// Release stayed within `CLICK_DRAG_THRESHOLD` of the press
    /// origin: treat as a click.
    Click,
    /// Release moved beyond `CLICK_DRAG_THRESHOLD`: treat as the tail
    /// of a drag.
    Drag,
    /// `on_pointer_up` was reached without a matching `on_pointer_down`
    /// (defensive, e.g. when another control captured the press).
    NoPress,
}

pub(super) struct InputState {
    phase: Cell<InputPhase>,
}

impl InputState {
    pub(super) fn new() -> Self {
        Self {
            phase: Cell::new(InputPhase::Idle),
        }
    }

    /// Latch a press at `origin`. Subsequent moves don't touch this;
    /// only [`set_idle`](Self::set_idle) or
    /// [`click_or_drag`](Self::click_or_drag) clear it.
    pub(super) fn set_pressed(&self, origin: ScreenPoint) {
        self.phase.set(InputPhase::Pressed { origin });
    }

    /// Force the phase back to idle without classifying. Used when an
    /// in-progress gesture is aborted (e.g. scene close, drag abort).
    pub(super) fn set_idle(&self) {
        self.phase.set(InputPhase::Idle);
    }

    /// Atomically take the current press origin (resetting to idle) and
    /// classify the release at `release` against `CLICK_DRAG_THRESHOLD`.
    pub(super) fn click_or_drag(&self, release: ScreenPoint) -> ClickOrDrag {
        match self.phase.replace(InputPhase::Idle) {
            InputPhase::Pressed { origin } => {
                let dx = release.x - origin.x;
                let dy = release.y - origin.y;
                if dx * dx + dy * dy <= CLICK_DRAG_THRESHOLD * CLICK_DRAG_THRESHOLD {
                    ClickOrDrag::Click
                } else {
                    ClickOrDrag::Drag
                }
            }
            InputPhase::Idle => ClickOrDrag::NoPress,
        }
    }
}

#[cfg(test)]
mod input_state_tests {
    use super::*;

    fn pt(x: f32, y: f32) -> ScreenPoint {
        ScreenPoint { x, y }
    }

    #[test]
    fn release_without_press_classifies_as_no_press() {
        let state = InputState::new();
        assert_eq!(state.click_or_drag(pt(10.0, 10.0)), ClickOrDrag::NoPress);
    }

    #[test]
    fn release_at_press_origin_is_click() {
        let state = InputState::new();
        state.set_pressed(pt(50.0, 50.0));
        assert_eq!(state.click_or_drag(pt(50.0, 50.0)), ClickOrDrag::Click);
    }

    #[test]
    fn release_just_below_threshold_is_click() {
        // (3.0, 2.0) has distance sqrt(13) ~= 3.606 < 4.0.
        let state = InputState::new();
        state.set_pressed(pt(0.0, 0.0));
        assert_eq!(state.click_or_drag(pt(3.0, 2.0)), ClickOrDrag::Click);
    }

    #[test]
    fn release_at_exact_threshold_is_click() {
        // Distance exactly 4.0 from the origin. The implementation
        // uses `<=`, so this is inclusive; the test pins that.
        let state = InputState::new();
        state.set_pressed(pt(0.0, 0.0));
        assert_eq!(state.click_or_drag(pt(4.0, 0.0)), ClickOrDrag::Click);
    }

    #[test]
    fn release_just_above_threshold_is_drag() {
        // (4.001, 0.0): distance 4.001 > 4.0.
        let state = InputState::new();
        state.set_pressed(pt(0.0, 0.0));
        assert_eq!(state.click_or_drag(pt(4.001, 0.0)), ClickOrDrag::Drag);
    }

    #[test]
    fn release_far_from_origin_is_drag() {
        let state = InputState::new();
        state.set_pressed(pt(0.0, 0.0));
        assert_eq!(state.click_or_drag(pt(100.0, 100.0)), ClickOrDrag::Drag);
    }

    #[test]
    fn nan_release_classifies_as_drag() {
        // NaN propagates through dx^2 + dy^2, so the `<=` comparison
        // is false and the gesture falls to the drag branch. This is
        // the safe default: a non-finite release can't be a click.
        let state = InputState::new();
        state.set_pressed(pt(0.0, 0.0));
        assert_eq!(state.click_or_drag(pt(f32::NAN, 0.0)), ClickOrDrag::Drag);
    }

    #[test]
    fn second_release_without_repress_is_no_press() {
        // First release consumes the latch; the next click_or_drag must
        // not reuse the prior origin (the double-up case).
        let state = InputState::new();
        state.set_pressed(pt(10.0, 10.0));
        assert_eq!(state.click_or_drag(pt(10.0, 10.0)), ClickOrDrag::Click);
        assert_eq!(state.click_or_drag(pt(10.0, 10.0)), ClickOrDrag::NoPress);
    }

    #[test]
    fn set_idle_clears_pending_press() {
        // Drag abort / scene reload uses set_idle to discard an
        // in-flight press. The next release must be NoPress.
        let state = InputState::new();
        state.set_pressed(pt(10.0, 10.0));
        state.set_idle();
        assert_eq!(state.click_or_drag(pt(10.0, 10.0)), ClickOrDrag::NoPress);
    }

    #[test]
    fn second_press_overrides_first_origin() {
        // If somehow two presses arrive without a release between them,
        // the most recent origin wins (the latch is a simple Cell).
        let state = InputState::new();
        state.set_pressed(pt(0.0, 0.0));
        state.set_pressed(pt(100.0, 100.0));
        // Released near the second origin, far from the first.
        assert_eq!(state.click_or_drag(pt(100.0, 100.0)), ClickOrDrag::Click);
    }
}
