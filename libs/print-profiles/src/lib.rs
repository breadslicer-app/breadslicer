/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Printer and material profile types for the resin-printing pipeline.
//!
//! This crate is the home of typed printer and material profile types
//! (mirroring the on-disk TOML profile files), the loading API that
//! reads them from explicit paths or from a configured library
//! directory, and the resolver that composes a chosen
//! (printer, material) pair plus per-print overrides into a
//! ready-to-consume resolved configuration.
//!
//! Consumers (the slicer crate, the slicer CLI, the eventual GUI) take
//! the resolved configuration and project it to their own input ports.
//! This crate does not depend on the slicer; it owns the printer- and
//! material-profile bounded context.
