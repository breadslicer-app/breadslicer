/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! From the readings of a printed XY coupon to corrections.
//!
//! A printed block of side `L` measures `w = s·L + 2·o`: `s` is the scale of
//! the print along that axis (the screen's pixels, the film, the resin
//! shrinking) and `o` how far each wall grows past where it was drawn (light
//! bleeding into the resin around the lit pixels). Blocks of several sizes
//! separate the two: the scale shows as a slope, the offset as what stays
//! when `L` goes to zero. One axis at a time, since a screen's pixels are not
//! always square.

use serde::{Deserialize, Serialize};

use crate::coupon::{BLOCKS_MM, HOLES_MM, PINS_MM, SLITS_MM};

/// A wall offset within this much is good.
pub const GOOD_OFFSET_MM: f32 = 0.025;
/// A scale within this much of 1 is good (0.3 %).
pub const GOOD_SCALE: f32 = 0.003;
/// Walls growing past this suggest too much light.
pub const OVER_OFFSET_MM: f32 = 0.04;
/// Walls shrinking past this suggest too little.
pub const UNDER_OFFSET_MM: f32 = -0.03;
/// A reading this far from what was drawn is likely a slip of the hand.
pub const SUSPICIOUS_MM: f32 = 1.0;

/// The thinnest slit still open, read on the printed coupon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Slit {
    /// Not read.
    #[default]
    NotRead,
    /// The thinnest that is still open, in mm.
    Open(f32),
    /// Every slit closed.
    AllClosed,
}

/// What a person reads off a printed XY coupon. Every reading is optional:
/// the fit uses what it gets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct XyReadings {
    /// Each block's width along X, in the order of [`BLOCKS_MM`].
    pub blocks_x: [Option<f32>; 3],
    /// Each block's width along Y, in the order of [`BLOCKS_MM`].
    pub blocks_y: [Option<f32>; 3],
    /// For each hole of [`HOLES_MM`], the thickest drill bit that goes in.
    pub hole_bits: [Option<f32>; 7],
    /// The thinnest pin standing whole.
    pub thinnest_pin: Option<f32>,
    /// Across X: the slits between the teeth laid side by side along X.
    pub slit_x: Slit,
    /// Across Y.
    pub slit_y: Slit,
    /// The column's width along X at the plate.
    pub foot_bottom: Option<f32>,
    /// The column's width along X near its top.
    pub foot_top: Option<f32>,
}

/// One axis' scale and wall offset.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AxisFit {
    /// Printed size over drawn size, walls aside.
    pub scale: f32,
    /// How far each wall grows (negative: shrinks), in mm.
    pub offset_mm: f32,
    /// How far the worst block sits from the fitted line, in mm.
    pub residual_mm: f32,
    /// How many blocks it fits.
    pub points: usize,
}

impl AxisFit {
    /// The scale compensation, in percent, that brings this axis back to its
    /// drawn size.
    pub fn compensation_pct(&self) -> f32 {
        (1.0 / self.scale - 1.0) * 100.0
    }

    fn good(&self) -> bool {
        self.offset_mm.abs() <= GOOD_OFFSET_MM && (self.scale - 1.0).abs() <= GOOD_SCALE
    }
}

/// What the light seems to do, judged by how the walls grow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Exposure {
    /// The walls stay where they were drawn.
    Right,
    /// The walls grow: too much light, or too long.
    Over,
    /// The walls shrink: too little.
    Under,
}

/// The overall reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Too few readings to say anything.
    NotEnough,
    /// Every axis within the good margins.
    Good,
    /// Off, and the numbers say by how much.
    Adjust,
    /// A reading looks mistyped: check it before trusting the rest.
    Suspicious,
}

/// A reading that looks wrong, and what it was.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Doubt {
    BlockX {
        side: f32,
        read: f32,
    },
    BlockY {
        side: f32,
        read: f32,
    },
    Foot {
        read: f32,
    },
    /// A drill bit wider than its hole.
    HoleBit {
        hole: f32,
        bit: f32,
    },
}

/// What the readings say.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct XyResult {
    pub x: Option<AxisFit>,
    pub y: Option<AxisFit>,
    /// How much the holes close, on average (diameter, mm).
    pub hole_closing_mm: Option<f32>,
    /// How far the first layers spread past the walls above them, per side.
    pub foot_mm: Option<f32>,
    pub thinnest_pin: Option<f32>,
    pub slit_x: Slit,
    pub slit_y: Slit,
    pub exposure: Option<Exposure>,
    pub verdict: Verdict,
    pub doubts: Vec<Doubt>,
}

/// Fits `w = s·L + 2·o` over the blocks that were read; needs two.
pub fn fit_axis(readings: &[Option<f32>; 3]) -> Option<AxisFit> {
    let points: Vec<(f32, f32)> = BLOCKS_MM
        .iter()
        .zip(readings)
        .filter_map(|(&l, w)| w.map(|w| (l, w)))
        .collect();
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f32;
    let mean_l = points.iter().map(|p| p.0).sum::<f32>() / n;
    let mean_w = points.iter().map(|p| p.1).sum::<f32>() / n;
    let var: f32 = points.iter().map(|p| (p.0 - mean_l).powi(2)).sum();
    if var <= 0.0 {
        return None;
    }
    let cov: f32 = points.iter().map(|p| (p.0 - mean_l) * (p.1 - mean_w)).sum();
    let scale = cov / var;
    let offset_mm = (mean_w - scale * mean_l) / 2.0;
    let residual_mm = points
        .iter()
        .map(|p| (p.1 - (scale * p.0 + 2.0 * offset_mm)).abs())
        .fold(0.0, f32::max);
    Some(AxisFit {
        scale,
        offset_mm,
        residual_mm,
        points: points.len(),
    })
}

impl XyReadings {
    /// What the readings say.
    pub fn result(&self) -> XyResult {
        let mut doubts = Vec::new();
        for (k, &side) in BLOCKS_MM.iter().enumerate() {
            if let Some(read) = self.blocks_x[k]
                && (read - side).abs() > SUSPICIOUS_MM
            {
                doubts.push(Doubt::BlockX { side, read });
            }
            if let Some(read) = self.blocks_y[k]
                && (read - side).abs() > SUSPICIOUS_MM
            {
                doubts.push(Doubt::BlockY { side, read });
            }
        }
        let column = crate::coupon::COLUMN_MM[0];
        for read in [self.foot_bottom, self.foot_top].into_iter().flatten() {
            if (read - column).abs() > SUSPICIOUS_MM {
                doubts.push(Doubt::Foot { read });
            }
        }
        let mut closing = Vec::new();
        for (k, &hole) in HOLES_MM.iter().enumerate() {
            if let Some(bit) = self.hole_bits[k] {
                if bit > hole + 0.05 {
                    doubts.push(Doubt::HoleBit { hole, bit });
                } else {
                    closing.push(hole - bit);
                }
            }
        }
        let hole_closing_mm =
            (!closing.is_empty()).then(|| closing.iter().sum::<f32>() / closing.len() as f32);
        let foot_mm = match (self.foot_bottom, self.foot_top) {
            (Some(bottom), Some(top)) => Some((bottom - top) / 2.0),
            _ => None,
        };

        let x = fit_axis(&self.blocks_x);
        let y = fit_axis(&self.blocks_y);
        let offsets: Vec<f32> = [x, y].iter().flatten().map(|f| f.offset_mm).collect();
        let exposure = (!offsets.is_empty()).then(|| {
            let o = offsets.iter().sum::<f32>() / offsets.len() as f32;
            if o > OVER_OFFSET_MM {
                Exposure::Over
            } else if o < UNDER_OFFSET_MM {
                Exposure::Under
            } else {
                Exposure::Right
            }
        });
        let verdict = if !doubts.is_empty() {
            Verdict::Suspicious
        } else if x.is_none() && y.is_none() {
            Verdict::NotEnough
        } else if [x, y].iter().flatten().all(AxisFit::good) {
            Verdict::Good
        } else {
            Verdict::Adjust
        };
        XyResult {
            x,
            y,
            hole_closing_mm,
            foot_mm,
            thinnest_pin: self.thinnest_pin,
            slit_x: self.slit_x,
            slit_y: self.slit_y,
            exposure,
            verdict,
            doubts,
        }
    }
}

/// The sizes a person picks from, so readings stay on what was printed.
pub fn pin_choices() -> &'static [f32] {
    &PINS_MM
}

/// The slit widths, thinnest first.
pub fn slit_choices() -> &'static [f32] {
    &SLITS_MM
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printed(scale: f32, offset: f32) -> [Option<f32>; 3] {
        BLOCKS_MM.map(|l| Some(scale * l + 2.0 * offset))
    }

    #[test]
    fn the_fit_separates_scale_from_offset() {
        let fit = fit_axis(&printed(1.004, 0.05)).unwrap();
        assert!((fit.scale - 1.004).abs() < 1e-5, "{fit:?}");
        assert!((fit.offset_mm - 0.05).abs() < 1e-5, "{fit:?}");
        assert!(fit.residual_mm < 1e-4);
        assert_eq!(fit.points, 3);
        assert!((fit.compensation_pct() + 0.398).abs() < 0.01);
    }

    #[test]
    fn two_blocks_are_enough_and_one_is_not() {
        let mut two = printed(0.998, -0.02);
        two[2] = None;
        assert!(fit_axis(&two).is_some());
        let mut one = two;
        one[1] = None;
        assert!(fit_axis(&one).is_none());
    }

    #[test]
    fn exact_prints_read_good() {
        let readings = XyReadings {
            blocks_x: printed(1.0, 0.0),
            blocks_y: printed(1.001, 0.01),
            ..Default::default()
        };
        let result = readings.result();
        assert_eq!(result.verdict, Verdict::Good);
        assert_eq!(result.exposure, Some(Exposure::Right));
    }

    #[test]
    fn walls_that_grow_point_at_too_much_light() {
        let readings = XyReadings {
            blocks_x: printed(1.0, 0.08),
            blocks_y: printed(1.0, 0.07),
            hole_bits: [None, None, None, Some(1.3), Some(1.8), None, None],
            foot_bottom: Some(8.6),
            foot_top: Some(8.1),
            ..Default::default()
        };
        let result = readings.result();
        assert_eq!(result.verdict, Verdict::Adjust);
        assert_eq!(result.exposure, Some(Exposure::Over));
        assert!((result.hole_closing_mm.unwrap() - 0.2).abs() < 1e-5);
        assert!((result.foot_mm.unwrap() - 0.25).abs() < 1e-5);
    }

    #[test]
    fn a_reading_far_off_is_doubted() {
        let mut blocks = printed(1.0, 0.0);
        blocks[1] = Some(18.0);
        let readings = XyReadings {
            blocks_x: blocks,
            ..Default::default()
        };
        let result = readings.result();
        assert_eq!(result.verdict, Verdict::Suspicious);
        assert_eq!(
            result.doubts,
            vec![Doubt::BlockX {
                side: 8.0,
                read: 18.0
            }]
        );
    }

    #[test]
    fn nothing_read_says_not_enough() {
        assert_eq!(XyReadings::default().result().verdict, Verdict::NotEnough);
    }
}
