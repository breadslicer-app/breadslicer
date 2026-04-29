/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use glam::Vec3;

/// World-aligned gizmo axis. The X/Y/Z directions are global +X, +Y,
/// +Z; a local-axis option is left as a future toggle.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const fn world_dir(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }
}

pub const ALL_AXES: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

/// In-plane orthonormal basis around `axis`, right-handed about the
/// axis (`axis × u = v`). Used by every rotate primitive so the
/// angular position returned by `drag_math::axis_plane_angle`
/// grows CCW about `+axis` (right-hand rule). Caller code that wants
/// the user's "screen clockwise = +rotation" convention flips the
/// sign at composition time, where the camera direction is also known.
pub(super) fn axis_basis(axis: Axis) -> (Vec3, Vec3) {
    match axis {
        Axis::X => (Vec3::Y, Vec3::Z),
        Axis::Y => (Vec3::Z, Vec3::X),
        Axis::Z => (Vec3::X, Vec3::Y),
    }
}

#[cfg(test)]
mod axis_tests {
    use super::*;

    #[test]
    fn world_dir_returns_basis_vectors() {
        assert_eq!(Axis::X.world_dir(), Vec3::X);
        assert_eq!(Axis::Y.world_dir(), Vec3::Y);
        assert_eq!(Axis::Z.world_dir(), Vec3::Z);
    }
}
