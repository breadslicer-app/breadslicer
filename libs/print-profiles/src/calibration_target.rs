/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The key of a material's calibration block.
//!
//! In the TOML form every block is `[per_printer."<id>"]`, where the
//! id is either a specific printer's or the generic machine's. The
//! two kinds of key play different roles in resolution (the generic
//! block applies to any printer without a block of its own), so they
//! are distinct variants here; the slug form is preserved on both
//! sides of serde.

use std::fmt;

use sc_print_primitives::{InvalidProfileId, PrinterId};
use serde::{Deserialize, Serialize};

use crate::generic_machine::generic_printer_id;

/// Key of a material's calibration block: a specific printer, or the
/// generic machine, whose block applies to any printer without its
/// own. `Generic` orders before every `Printer`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub enum CalibrationTarget {
    Generic,
    Printer(SpecificPrinterId),
}

/// A printer id that is not the generic machine's. Only
/// [`CalibrationTarget`]'s constructors build one, so a
/// `CalibrationTarget::Printer` never carries the generic id.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpecificPrinterId(PrinterId);

impl SpecificPrinterId {
    pub fn as_printer_id(&self) -> &PrinterId {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl CalibrationTarget {
    /// The target a block keyed by `id` belongs to: `Generic` for the
    /// generic machine's id, `Printer` otherwise.
    pub fn from_printer(id: &PrinterId) -> Self {
        Self::from(id.clone())
    }

    /// The printer id this target is keyed by in the TOML form; the
    /// generic machine's id for `Generic`.
    pub fn printer_id(&self) -> &PrinterId {
        match self {
            Self::Generic => generic_printer_id(),
            Self::Printer(id) => id.as_printer_id(),
        }
    }

    pub fn as_str(&self) -> &str {
        self.printer_id().as_str()
    }
}

impl From<PrinterId> for CalibrationTarget {
    fn from(id: PrinterId) -> Self {
        if &id == generic_printer_id() {
            Self::Generic
        } else {
            Self::Printer(SpecificPrinterId(id))
        }
    }
}

impl TryFrom<String> for CalibrationTarget {
    type Error = InvalidProfileId;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        PrinterId::try_from(value).map(Self::from)
    }
}

impl From<CalibrationTarget> for String {
    fn from(target: CalibrationTarget) -> Self {
        match target {
            CalibrationTarget::Generic => generic_printer_id().as_str().to_owned(),
            CalibrationTarget::Printer(SpecificPrinterId(id)) => id.into(),
        }
    }
}

impl fmt::Display for CalibrationTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
