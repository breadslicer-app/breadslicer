/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The still images a printer file carries for the machine's own
//! screen.
//!
//! Every one of these containers reserves room for 2 renderings of
//! the plate at sizes it fixes: a small one for the file browser's
//! list and a large one for the entry it opens. Producing the pixels
//! needs a renderer and a scene, neither of which this crate knows
//! about, so the caller renders and this crate packs.

use std::fmt;

/// Channels per pixel of a [`PreviewImage`].
const CHANNELS: usize = 3;

/// One rendering of the plate, 8 bits per channel, rows top to
/// bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewImage {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

/// Returned when a preview's pixels do not fill its stated size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelCountMismatch {
    pub width: u32,
    pub height: u32,
    /// Length of the buffer that was offered.
    pub bytes: usize,
}

impl fmt::Display for PixelCountMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a {} x {} preview holds {} bytes of RGB, not {}",
            self.width,
            self.height,
            u64::from(self.width) * u64::from(self.height) * CHANNELS as u64,
            self.bytes,
        )
    }
}

impl std::error::Error for PixelCountMismatch {}

impl PreviewImage {
    /// Take ownership of one rendering, 3 bytes per pixel in R, G, B
    /// order, rows top to bottom and pixels left to right.
    ///
    /// # Errors
    ///
    /// [`PixelCountMismatch`] when `rgb` is not exactly 3 bytes per
    /// pixel of the stated size.
    pub fn new(width: u32, height: u32, rgb: Vec<u8>) -> Result<Self, PixelCountMismatch> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(CHANNELS));
        if expected != Some(rgb.len()) {
            return Err(PixelCountMismatch {
                width,
                height,
                bytes: rgb.len(),
            });
        }
        Ok(Self { width, height, rgb })
    }

    /// Report the width in pixels.
    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Report the height in pixels.
    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Borrow the pixels, 3 bytes each.
    #[inline]
    pub fn rgb(&self) -> &[u8] {
        &self.rgb
    }
}

/// The 2 renderings a printer file carries.
#[derive(Clone, Debug, PartialEq)]
pub struct PrintPreviews {
    /// The file browser's list entry.
    pub small: PreviewImage,
    /// The picture that entry opens.
    pub large: PreviewImage,
}

/// Returned when a rendering does not have the size a container
/// fixes for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewSizeMismatch {
    pub expected_width: u32,
    pub expected_height: u32,
    pub actual_width: u32,
    pub actual_height: u32,
}

impl fmt::Display for PreviewSizeMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "this container fixes the preview at {} x {} pixels, and the rendering is {} x {}",
            self.expected_width, self.expected_height, self.actual_width, self.actual_height,
        )
    }
}

impl std::error::Error for PreviewSizeMismatch {}
