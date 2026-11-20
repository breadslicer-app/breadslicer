/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The range adaptive layer heights keep to (#60).

use crate::length::PositiveFiniteMm;

/// The range an adaptive schedule's layers keep to: the thinnest layer, which
/// a flat face takes, and the thickest, which a wall takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct AdaptiveLayers {
    /// The thinnest layer.
    pub min_mm: PositiveFiniteMm,
    /// The thickest layer.
    pub max_mm: PositiveFiniteMm,
}

impl AdaptiveLayers {
    /// The range a print starts from when adaptive layers are first turned
    /// on: a quarter to a tenth of a millimetre.
    pub const DEFAULT: Self = Self {
        min_mm: match PositiveFiniteMm::new(0.025) {
            Ok(value) => value,
            Err(_) => panic!("0.025 mm is a length"),
        },
        max_mm: match PositiveFiniteMm::new(0.1) {
            Ok(value) => value,
            Err(_) => panic!("0.1 mm is a length"),
        },
    };

    /// The range from `min` to `max`, the two swapped when `min` is the
    /// thicker.
    pub fn new(min: PositiveFiniteMm, max: PositiveFiniteMm) -> Self {
        if min.value() <= max.value() {
            Self {
                min_mm: min,
                max_mm: max,
            }
        } else {
            Self {
                min_mm: max,
                max_mm: min,
            }
        }
    }
}

#[cfg(test)]
mod adaptive_layers_tests {
    use super::*;

    fn mm(value: f64) -> PositiveFiniteMm {
        PositiveFiniteMm::new(value).unwrap()
    }

    #[test]
    fn a_range_keeps_its_thinnest_first() {
        assert_eq!(
            AdaptiveLayers::new(mm(0.1), mm(0.03)),
            AdaptiveLayers::new(mm(0.03), mm(0.1))
        );
        assert_eq!(AdaptiveLayers::new(mm(0.03), mm(0.1)).min_mm, mm(0.03));
        assert!(AdaptiveLayers::DEFAULT.min_mm.value() < AdaptiveLayers::DEFAULT.max_mm.value());
    }
}
