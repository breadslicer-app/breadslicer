/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! How a printer file stores the plate's image: as the plate lies, or
//! turned a quarter turn.
//!
//! Profiles describe the plate as it sits in the machine, X across the
//! front and Y in depth, and the scene lays it out the same way. Some
//! panels run the other way round: their files store the plate's image
//! turned 90 degrees, so the image is as many pixels wide as the plate
//! is deep. [`PanelRotation`] names that turn. The mirror flags apply
//! first, to the plate's image; the turn comes after them.

/// The turn between the plate's image and the image a printer file
/// stores. The plate's image is the plate seen from above, from behind:
/// its first row is the plate's -Y (front) edge, its first column the
/// +X edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum PanelRotation {
    /// The file stores the plate's image as it lies.
    #[default]
    #[cfg_attr(feature = "serde", serde(rename = "none"))]
    None,
    /// The file stores the plate's image turned a quarter turn
    /// counter-clockwise: the plate's -X edge is the stored image's
    /// first row and its -Y edge the stored image's first column.
    #[cfg_attr(feature = "serde", serde(rename = "ccw90"))]
    Ccw90,
}

impl PanelRotation {
    /// Report whether the stored image keeps the plate's axes, so a
    /// profile can leave the setting out.
    #[inline]
    pub fn is_none(&self) -> bool {
        *self == Self::None
    }

    /// Report whether the stored image's width runs along the plate's Y.
    #[inline]
    pub fn swaps_axes(self) -> bool {
        match self {
            Self::None => false,
            Self::Ccw90 => true,
        }
    }

    /// Order a plate-frame `(x, y)` pair as the stored image's
    /// `(width, height)`.
    #[inline]
    pub fn to_panel<T>(self, (x, y): (T, T)) -> (T, T) {
        if self.swaps_axes() { (y, x) } else { (x, y) }
    }
}
