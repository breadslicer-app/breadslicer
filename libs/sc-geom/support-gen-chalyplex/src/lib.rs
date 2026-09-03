/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Reserved for the chalyplex support-generation algorithm family:
//! Sobolev / weak-formulation based support generation for STL-like
//! meshes with sharp edges (see `../CITATION.cff`). Under development
//! elsewhere; empty here on purpose.
//!
//! Once it lands, this crate implements `support-gen-family-dispatch`'s
//! `GeomAlgorithm` trait the same way `support-gen-trivial` does
//! today, so `support-gen-family-dispatch` and the main app need no changes
//! to pick it up. It is expected to grow its own internal,
//! enum-based variants (multiple support types, per the constraint
//! set a print/material calls for) rather than becoming several
//! sibling crates.
