/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! What a machine expects when the resin does not say.

use serde::{Deserialize, Serialize};

/// A machine's own figures, used only where nothing calibrated for the
/// resin supplies one.
///
/// These are the manufacturer's expectations for the machine rather
/// than measurements for any resin, so a print that runs on one is
/// reported as uncalibrated even though the number came from a profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct PrinterDefaults {
    /// How many layers this machine expects to be exposed for the
    /// bottom time, when no resin states its own.
    pub bottom_layer_count: Option<u32>,
}

#[cfg(test)]
mod printer_defaults_tests {
    use super::*;

    #[test]
    fn a_machine_that_states_nothing_parses_to_no_figures() {
        let defaults: PrinterDefaults = toml_edit::de::from_str("").unwrap();

        assert_eq!(defaults, PrinterDefaults::default());
        assert_eq!(defaults.bottom_layer_count, None);
    }

    #[test]
    fn a_stated_bottom_layer_count_is_read() {
        let defaults: PrinterDefaults = toml_edit::de::from_str("bottom_layer_count = 6").unwrap();

        assert_eq!(defaults.bottom_layer_count, Some(6));
    }
}
