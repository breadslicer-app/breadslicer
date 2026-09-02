/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! [`PrintPlate`]: the physical machine context a geometry operation
//! may need, independent of any SoulCrafted profile format. This
//! crate stays standalone (see the crate README), so it does not
//! depend on `sc-print-profiles`/`sc-print-primitives`; the driver
//! projects its own `PrinterHardware` into a `PrintPlate` instead of
//! this crate reading a printer profile directly.

use sc_mesh_core::Vector3D;

use crate::boundary::BoundaryShape;

/// The printer's physical build envelope and which way is "down" off
/// the plate. Distinct from a job's [`BoundaryShape`] specs/settings, which
/// may be a tighter region chosen for one print-job alone.
/// This is the chosen 3D-printer machine's own limits (in-itself).
#[derive(Clone, Debug, PartialEq)]
pub struct PrintPlate {
    pub boundary: BoundaryShape,
    /// Outward normal of the plate surface. Not assumed to be
    /// parallel to gravity: non-planar and non-horizontal printing are
    /// in scope for this crate even though today's callers are
    /// conventional printers (see [`crate::handler::SupportStructureSpecs::gravity`]).
    pub plate_normal: Vector3D<f64>,
}
