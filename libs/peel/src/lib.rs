/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Approximate physics of a bottom-up resin print's peel (#11).
//!
//! Each layer cures against the release film at the bottom of the vat, and
//! the platform lifts to peel it off. The film pulls on what was just cured
//! with a force that grows with its area ([`PeelModel`]); cavities closed
//! towards the plate and open towards the film add suction. The printed body
//! that layer belongs to takes that pull through the support tips that hold
//! it, each its share ([`TipGroup`]).
//!
//! # Units
//!
//! Lengths in millimetres, areas in square millimetres, stresses in kPa
//! (millinewtons per square millimetre) for the peel and MPa (newtons per
//! square millimetre) for the tips' strength, forces in newtons.
//!
//! # Not calibrated
//!
//! Every coefficient is a [`Band`], low / mid / high, from published
//! measurements on other machines and films. Until test prints calibrate
//! them, the numbers compare layers, parts and tips with one another (every
//! force scales with the peel stress, so their order holds), and say nothing
//! absolute about whether a support holds. A frontend says so wherever it
//! shows them.

mod curve;
mod model;
mod perimeter;
mod share;

pub use curve::{LayerMeasure, LayerPeel, PeelCurve, SPIKE_WINDOW};
pub use model::{
    Band, DEFAULT_LATERAL_SHARE, FLEXIBLE_SPEED_EXPONENT, PeelModel, REFERENCE_SPEED_MM_MIN,
    ReleaseFilm,
};
pub use perimeter::{PerimeterMeter, perimeter_of_rows};
pub use share::{TipGroup, TipLoadCase, TipSums};
