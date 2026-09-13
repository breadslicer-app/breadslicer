/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The ChiTuBox CTB container family.
//!
//! One skeleton of offset-linked sections carries several revisions,
//! which share the layer codec and differ in their fixed blocks and in
//! whether the settings block is encrypted. Everything multi-byte is
//! little-endian except a layer run's length.

pub mod cipher;
pub mod layer_xor;
pub mod rle15;
pub mod rle7;
pub mod v5;
pub mod wire;

use crate::preview::{PreviewSize, PreviewSizes};

/// The sizes these writers render at. The container fixes neither,
/// since every preview record carries its own width and height, so
/// these are the writers' own choice: they are what the
/// reverse-engineering sources report and what third-party readers
/// expect to find.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 200,
        height: 125,
    },
    large: PreviewSize {
        width: 400,
        height: 300,
    },
};

/// Every revision carries a single light-off delay beside 3 separate
/// waits, with nothing to select between them, and a layer program
/// holds the waits. Every delay slot is written zero so that only one
/// of the 2 schemes describes the pause.
pub(crate) const LIGHT_OFF_SEC: f32 = 0.0;

/// The blocks report material in millilitres and grams per
/// millilitre, while a print program measures it in cubic
/// millimetres.
pub(crate) const MM3_PER_ML: f64 = 1000.0;

#[cfg(test)]
mod tests_cipher;
#[cfg(test)]
mod tests_layer_xor;
#[cfg(test)]
mod tests_rle15;
#[cfg(test)]
mod tests_rle7;
