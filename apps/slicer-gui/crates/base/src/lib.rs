/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! The base of the Breadslicer GUI crates: the event trail
//! ([`breadcrumbs`]), the scrubbing that keeps personal paths out of it
//! ([`scrub`]), the startup timings ([`startup_timing`]) and the translation
//! macro [`ctr!`].

pub mod breadcrumbs;
mod i18n;
pub mod scrub;
pub mod startup_timing;
