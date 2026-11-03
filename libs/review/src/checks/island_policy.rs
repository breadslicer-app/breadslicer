/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Which islands are worth a support (#163): every threshold of the island
//! policy, in one place. The island check reads them through
//! [`IslandOptions`](super::IslandOptions), whose defaults they are; the
//! policy is on by default for every preset.
//!
//! What they rest on, measured on the islands Fix All could not support on
//! three minis of a real plate (0.05 mm layers, 17 x 25 µm pixels): 19
//! islands of 4 to 112 pixels (0.002 to 0.047 mm²), 14 of them one layer
//! high and 18 of them joined to the body within three layers, the next
//! layer covering 88 to 100 % of them, with model or support 0.05 to 0.9 mm
//! straight below. No support fits there, and none would help:
//!
//! - **The next layer lifts a speck.** The first exposure of an island cures
//!   a flake of its area on the vat film; the next layer cures right over it
//!   and, being part of the body already, peels it off with it. The worst
//!   that comes of it is a one-layer dent of a tenth of a millimetre. The
//!   danger is an island that keeps growing for many layers before it joins
//!   anything, so the rule weighs how high and how much it grows, not only
//!   its first layer.
//! - **A tip on a speck does harm.** The smallest tip prints a contact of
//!   about 0.3 mm ([`TIP_CONTACT_MM2`]), larger than most such islands: it
//!   covers the island whole and tears it off at removal, and it leaves a
//!   mark on two surfaces of the model.
//! - **Light cures deeper than a layer.** The exposure of a layer cures down
//!   past its own thickness ([`CURE_DEPTH_MM`]), so solid one layer below a
//!   pixel is reached through the layer of liquid between them.

/// The contact a support tip really prints, in mm²: a disc of 0.3 mm. The
/// thinnest tip we make is 0.18 mm, but measured on sliced layers every tip
/// prints a contact of 0.29 to 0.38 mm. An island section no larger than
/// this is no larger than the tip that would hold it.
pub const TIP_CONTACT_MM2: f64 = 0.07;

/// The highest a minimal island grows before something sound holds it, in
/// mm, its layers included (#402): 0.10, two layers of 0.05 mm, one of
/// 0.1 mm, three of 0.03 mm.
pub const MINIMAL_ISLAND_HEIGHT_MM: f64 = 0.10;

/// The resin a minimal island gathers before something sound holds it, in
/// mm³ (#402): under 0.01. The review already erases a loose speck under a
/// tenth of that; the islands a support could not reach gathered 0.0001 to
/// 0.004 mm³.
pub const MINIMAL_ISLAND_VOLUME_MM3: f64 = 0.01;

/// The largest section of a minimal island, in mm² (#402): the tip's
/// contact.
pub const MINIMAL_ISLAND_SECTION_MM2: f64 = TIP_CONTACT_MM2;

/// How deep the exposure of a layer cures, in mm, its own thickness
/// included (#403): 0.11, modelled for a normal exposure (cure depth = Dp ·
/// ln(E/Ec), Dp about 70 µm), not yet calibrated on a printer. Solid that
/// many layers below a pixel holds it: `floor(depth / layer) - 1` layers of
/// liquid in between, one at 0.05 mm, two at 0.03 mm, none at 0.1 mm. It
/// will come from the resin's measured cure depth (#98).
pub const CURE_DEPTH_MM: f64 = 0.11;

/// The most layers of liquid [`CURE_DEPTH_MM`] bridges, whatever the layer
/// height: it bounds how many layers below each layer the check reads.
pub const MAX_CURED_GAP_LAYERS: usize = 4;

/// The largest piece kept with the part it touches across a weak joint, in
/// mm² (#404): the tip's contact. Pulled apart by a joint of a pixel or two
/// that the light's blur widens anyway, a piece this small is part of what
/// it touches: the peel force on it does not break the joint.
pub const WEAK_JOINT_KEEP_MM2: f64 = TIP_CONTACT_MM2;

/// The thickest layer of liquid between an island and model below it that
/// the review offers to close by curing it ("Join to below", #235), in mm:
/// 0.10, two layers of 0.05 mm. No support fits in such a gap, and closing
/// it is what the overcure nearly does on its own. Only over model, never
/// over a support: that would glue the part to its support.
pub const JOIN_GAP_MM: f64 = 0.10;

/// The share of an island's first layer that model within [`JOIN_GAP_MM`]
/// below must cover for "Join to below" to hold it: the margin of cured
/// model under any piece.
pub const JOIN_MIN_FRACTION: f64 = super::MIN_SUPPORTED_FRACTION;

/// How much higher than its first layer a growing island's support may aim
/// at the same face, in mm (#405): 0.10, two layers of 0.05 mm. When the
/// first layer is smaller than [`TIP_CONTACT_MM2`], the support goes where
/// the island first reaches that size, no higher than this; what hangs
/// below it is then a minimal island.
pub const SUPPORT_CLIMB_MM: f64 = 0.10;

/// The largest island the review leaves as a note once the support engine
/// found no way to it, in mm² of its largest section (#163): 1.0, a square
/// millimetre. This applies only after the engine tried and failed: where a
/// support fits, the island gets one. Where none fits, an island never
/// larger than this is too small to be worth a warning: the engine found no
/// valid, convenient spot for a tip, and nothing more can be done for it
/// from the review. A larger one stays a warning: something that size
/// hangs with nothing under it. On a 46 mm pre-supported mini, the islands
/// no support reached grew to 0.003 to 0.80 mm².
pub const NO_SUPPORT_NOTE_MM2: f64 = 1.0;

/// Slack on the height comparisons, in mm: layer heights summed in floating
/// point.
pub const HEIGHT_TOLERANCE_MM: f64 = 1e-6;
