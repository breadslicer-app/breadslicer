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
