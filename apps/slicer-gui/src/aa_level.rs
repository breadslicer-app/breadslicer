/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Mapping between the anti-aliasing `ComboBox` index in the Slicing
//! Preferences page and the domain `AaLevel`.
//!
//! The combo model is a static four-entry list (see
//! `ui/preferences/slicing.slint`): index 0 is `Off`, then `X2` / `X4`
//! / `X8`. These two functions are the single place the order is encoded
//! on the Rust side, so the callback wiring and the restore path stay in
//! agreement with the Slint list.

use sc_print_primitives::AaLevel;

/// Position of an `AaLevel` in the anti-aliasing combo model.
pub(crate) fn aa_combo_index(level: AaLevel) -> i32 {
    match level {
        AaLevel::Off => 0,
        AaLevel::X2 => 1,
        AaLevel::X4 => 2,
        AaLevel::X8 => 3,
    }
}

/// Map a `ComboBox.current-index` back to the typed `AaLevel`.
/// Out-of-range values fall back to `Off`.
pub(crate) fn aa_level_from_combo_index(index: i32) -> AaLevel {
    match index {
        1 => AaLevel::X2,
        2 => AaLevel::X4,
        3 => AaLevel::X8,
        _ => AaLevel::Off,
    }
}

#[cfg(test)]
mod aa_level_tests {
    use super::*;

    #[test]
    fn every_level_round_trips_through_its_index() {
        for level in [AaLevel::Off, AaLevel::X2, AaLevel::X4, AaLevel::X8] {
            assert_eq!(aa_level_from_combo_index(aa_combo_index(level)), level);
        }
    }

    #[test]
    fn off_is_index_zero() {
        assert_eq!(aa_combo_index(AaLevel::Off), 0);
        assert_eq!(aa_level_from_combo_index(0), AaLevel::Off);
    }

    #[test]
    fn out_of_range_index_falls_back_to_off() {
        assert_eq!(aa_level_from_combo_index(-1), AaLevel::Off);
        assert_eq!(aa_level_from_combo_index(99), AaLevel::Off);
    }
}
