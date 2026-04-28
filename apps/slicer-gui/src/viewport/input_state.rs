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
    /// origin — treat as a click.
    Click,
    /// Release moved beyond `CLICK_DRAG_THRESHOLD` — treat as the tail
    /// of a drag.
    Drag,
    /// `on_pointer_up` was reached without a matching `on_pointer_down`
    /// (defensive — e.g. when another control captured the press).
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
