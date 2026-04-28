/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(super) struct ObjectIdx(pub(super) usize);

impl ObjectIdx {
    pub(super) fn as_usize(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Selection {
    None,
    One(ObjectIdx),
}

impl Selection {
    pub(super) fn primary(self) -> Option<ObjectIdx> {
        match self {
            Selection::None => None,
            Selection::One(i) => Some(i),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ScreenPoint {
    pub(super) x: f32,
    pub(super) y: f32,
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
