/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! How well a material's calibration blocks cover a printer: the
//! same rule the resolver applies when it picks a block, stated over
//! the block keys alone so callers can classify a material without
//! resolving it.

use sc_print_primitives::PrinterId;

use crate::generic_machine::generic_printer_id;

/// How well a set of calibration block keys covers a printer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibrationCoverage {
    /// A block for the printer itself exists.
    Specific,
    /// No block for the printer, but the generic block applies as a
    /// fallback.
    GenericFallback,
    /// No block applies to the printer.
    None,
}

impl CalibrationCoverage {
    /// Coverage of `printer` by a set of calibration block keys (a
    /// material's own plus its extension's). With no printer, only the
    /// generic block can cover.
    pub fn of<'k>(
        keys: impl IntoIterator<Item = &'k PrinterId>,
        printer: Option<&PrinterId>,
    ) -> Self {
        let mut has_generic = false;
        for key in keys {
            if Some(key) == printer {
                return Self::Specific;
            }
            if key == generic_printer_id() {
                has_generic = true;
            }
        }
        if has_generic {
            Self::GenericFallback
        } else {
            Self::None
        }
    }
}
