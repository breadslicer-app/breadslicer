/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Top-level resolver output: a (printer, resolved material) pair
//! ready for the slicer / CLI / GUI to project to their own input
//! ports.
//!
//! [`ResolvedPrintConfig`] bundles the printer profile (used
//! verbatim, since the printer-side fields are not subject to the
//! same fall-through composition as material values), the resolved
//! material values, the diagnostic trail of value-source records,
//! and any non-fatal warnings emitted during resolution.

use crate::material_resolved::MaterialResolved;
use crate::override_record::OverrideRecord;
use crate::printer_profile::PrinterProfile;
use crate::resolve_warning::ResolveWarning;

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPrintConfig {
    pub printer: PrinterProfile,
    pub material_resolved: MaterialResolved,
    pub overrides_applied: Vec<OverrideRecord>,
    pub warnings: Vec<ResolveWarning>,
}
