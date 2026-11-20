/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The peel model: how hard the film pulls on a cured region (#240).
//!
//! For one solid region of one layer, of area `A` and perimeter `P`, with
//! `A_cup` of cavities in it closed towards the plate:
//!
//! ```text
//! F = (A·(σ + κ·A/P) + Δp·A_cup) · s(v)
//! ```
//!
//! - `σ`, the peel stress of the film: on flexible films the force measured
//!   per cured area is close to linear in the area, 12 to 19 kPa;
//! - `κ ≥ 0`, a shape term: at equal area, compact shapes (less perimeter)
//!   pull more, since the peel front starts at the edge. It starts at 0
//!   until prints measure it;
//! - `Δp`, the pressure drop under a suction cup: resin cannot flow into the
//!   cavity as the film pulls away, up to about an atmosphere;
//! - `s(v) = (v / v_ref)^n`, the lift speed: almost flat on flexible films.

/// The release film at the bottom of the vat (#239).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ReleaseFilm {
    /// Plain FEP, the most common, and the default.
    #[default]
    Fep,
    /// nFEP.
    Nfep,
    /// ACF.
    Acf,
    /// PFA.
    Pfa,
}

impl ReleaseFilm {
    /// Every film, in a stable order.
    pub const ALL: [Self; 4] = [Self::Fep, Self::Nfep, Self::Acf, Self::Pfa];

    /// A short name (`"FEP"`).
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fep => "FEP",
            Self::Nfep => "nFEP",
            Self::Acf => "ACF",
            Self::Pfa => "PFA",
        }
    }

    /// The film's typical peel stress in kPa, the middle of its band.
    const fn mid_stress_kpa(self) -> f64 {
        match self {
            // 13-19 kPa measured on 50-150 µm FEP; PFA is a close relative
            // with no measurement found, so it reads as FEP.
            Self::Fep | Self::Pfa => 15.0,
            Self::Nfep => 13.0,
            Self::Acf => 12.0,
        }
    }
}

/// A value known only within a range until prints calibrate it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    /// The low end.
    pub low: f64,
    /// The most likely value, the one to show.
    pub mid: f64,
    /// The high end.
    pub high: f64,
}

impl Band {
    /// The band `low..=high` around `mid`.
    #[inline]
    pub const fn new(low: f64, mid: f64, high: f64) -> Self {
        Self { low, mid, high }
    }

    /// A band of one value.
    #[inline]
    pub const fn exact(value: f64) -> Self {
        Self::new(value, value, value)
    }

    /// Each end times `factor`.
    #[inline]
    #[must_use]
    pub fn scaled(self, factor: f64) -> Self {
        Self::new(self.low * factor, self.mid * factor, self.high * factor)
    }

    /// The ends added one to one.
    #[inline]
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self::new(
            self.low + other.low,
            self.mid + other.mid,
            self.high + other.high,
        )
    }
}

/// Default of [`PeelModel::reference_speed_mm_min`]: 60 mm/min, 1 mm/s.
pub const REFERENCE_SPEED_MM_MIN: f64 = 60.0;

/// Default of [`PeelModel::speed_exponent`] on a flexible film. On FEP the
/// fracture energy of the peel grows only 1.2 times from 0.1 to 1.5 mm/s,
/// a fifteenfold speed-up: `ln 1.2 / ln 15`.
pub const FLEXIBLE_SPEED_EXPONENT: f64 = 0.067;

/// Default of [`PeelModel::lateral_share`]: 5 %, the middle of the 0-10 %
/// assumed until prints measure it.
pub const DEFAULT_LATERAL_SHARE: f64 = 0.05;

/// The coefficients of the peel, for one film.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeelModel {
    /// The film they are for.
    pub film: ReleaseFilm,
    /// `σ`, the peel stress per cured area, in kPa.
    pub stress_kpa: Band,
    /// `κ`, the shape term, in kPa per millimetre of area over perimeter.
    /// 0 until prints measure it.
    pub shape_kpa_per_mm: f64,
    /// `Δp`, the suction under a cup, in kPa.
    pub cup_kpa: Band,
    /// `n` of the speed factor `(v / v_ref)^n`.
    pub speed_exponent: f64,
    /// `v_ref`, the lift speed the stresses are for, in mm/min.
    pub reference_speed_mm_min: f64,
    /// The share of a layer's pull that comes sideways, as the film slides
    /// and flexes: it bends the tips that hold the body.
    pub lateral_share: f64,
    /// The strength of a support tip's neck in the vat, in MPa, with the
    /// stress concentration of its narrowing taken in: what a tip of
    /// contact area `a` holds is `a` times this. Resin datasheets give
    /// 37-38 MPa for a washed, dry green part; in the vat it is weaker, and
    /// the neck concentrates the stress about twice.
    pub tip_strength_mpa: Band,
}

impl Default for PeelModel {
    fn default() -> Self {
        Self::for_film(ReleaseFilm::default())
    }
}

impl PeelModel {
    /// The coefficients for `film`, from published measurements: its peel
    /// stress from two thirds to five thirds of its typical value (10 / 15 /
    /// 25 kPa on FEP), cups from 20 to 100 kPa.
    pub fn for_film(film: ReleaseFilm) -> Self {
        let mid = film.mid_stress_kpa();
        Self {
            film,
            stress_kpa: Band::new(mid * 2.0 / 3.0, mid, mid * 5.0 / 3.0),
            shape_kpa_per_mm: 0.0,
            cup_kpa: Band::new(20.0, 50.0, 100.0),
            speed_exponent: FLEXIBLE_SPEED_EXPONENT,
            reference_speed_mm_min: REFERENCE_SPEED_MM_MIN,
            lateral_share: DEFAULT_LATERAL_SHARE,
            tip_strength_mpa: Band::new(4.0, 8.0, 16.0),
        }
    }

    /// The factor a lift at `speed_mm_min` puts on every force: 1 at the
    /// reference speed, and for a speed that is not one (a vat that tilts
    /// instead of lifting).
    pub fn speed_factor(&self, speed_mm_min: Option<f64>) -> f64 {
        match speed_mm_min {
            Some(speed) if speed.is_finite() && speed > 0.0 => {
                (speed / self.reference_speed_mm_min).powf(self.speed_exponent)
            }
            _ => 1.0,
        }
    }

    /// The pull of a region of `area_mm2`, with `perimeter_mm` of edge when
    /// it is known, and `cup_mm2` of suction cup, in newtons, at the
    /// reference speed. A perimeter that is not known, or not positive,
    /// leaves the shape term out.
    pub fn region_force_n(&self, area_mm2: f64, perimeter_mm: Option<f64>, cup_mm2: f64) -> Band {
        let area = area_mm2.max(0.0);
        let shape = match perimeter_mm {
            Some(perimeter) if perimeter > 0.0 && self.shape_kpa_per_mm > 0.0 => {
                self.shape_kpa_per_mm * area / perimeter
            }
            _ => 0.0,
        };
        // kPa x mm² = mN.
        let peel = Band::new(
            self.stress_kpa.low + shape,
            self.stress_kpa.mid + shape,
            self.stress_kpa.high + shape,
        )
        .scaled(area / 1000.0);
        peel.plus(self.cup_kpa.scaled(cup_mm2.max(0.0) / 1000.0))
    }

    /// The pull of a cup of `cup_mm2` alone, in newtons.
    pub fn cup_force_n(&self, cup_mm2: f64) -> Band {
        self.cup_kpa.scaled(cup_mm2.max(0.0) / 1000.0)
    }

    /// What a tip whose contact has radius `radius_mm` holds, in newtons.
    pub fn tip_capacity_n(&self, radius_mm: f64) -> Band {
        let area = std::f64::consts::PI * radius_mm.max(0.0).powi(2);
        self.tip_strength_mpa.scaled(area)
    }
}

#[cfg(test)]
mod model_tests {
    use super::*;

    #[test]
    fn a_square_centimetre_of_fep_pulls_one_and_a_half_newtons() {
        let model = PeelModel::for_film(ReleaseFilm::Fep);
        let force = model.region_force_n(100.0, None, 0.0);
        assert!((force.mid - 1.5).abs() < 1e-12);
        assert!((force.low - 1.0).abs() < 1e-12);
        assert!((force.high - 2.5).abs() < 1e-12);
    }

    #[test]
    fn force_is_linear_in_the_area_without_a_shape_term() {
        let model = PeelModel::default();
        let one = model.region_force_n(37.0, Some(20.0), 0.0).mid;
        let three = model.region_force_n(111.0, Some(60.0), 0.0).mid;
        assert!((three - 3.0 * one).abs() < 1e-12);
    }

    #[test]
    fn the_shape_term_pulls_more_on_compact_shapes() {
        let model = PeelModel {
            shape_kpa_per_mm: 2.0,
            ..PeelModel::default()
        };
        // Equal area, a disc against a long strip.
        let area = 100.0;
        let disc =
            model.region_force_n(area, Some(2.0 * (std::f64::consts::PI * area).sqrt()), 0.0);
        let strip = model.region_force_n(area, Some(2.0 * (100.0 + 1.0)), 0.0);
        assert!(disc.mid > strip.mid);
        assert!(strip.mid > model.region_force_n(area, None, 0.0).mid);
    }

    #[test]
    fn a_cup_adds_its_suction() {
        let model = PeelModel::default();
        let plain = model.region_force_n(50.0, None, 0.0);
        let cupped = model.region_force_n(50.0, None, 20.0);
        assert!((cupped.mid - plain.mid - 20.0 * 50.0 / 1000.0).abs() < 1e-12);
        assert_eq!(model.cup_force_n(20.0).high, 2.0);
    }

    #[test]
    fn flexible_films_are_almost_flat_in_speed() {
        let model = PeelModel::default();
        assert_eq!(model.speed_factor(Some(60.0)), 1.0);
        assert_eq!(model.speed_factor(None), 1.0);
        assert_eq!(model.speed_factor(Some(0.0)), 1.0);
        let fast = model.speed_factor(Some(900.0));
        assert!(fast > 1.15 && fast < 1.25, "{fast}");
    }

    #[test]
    fn every_film_has_an_ordered_band() {
        for film in ReleaseFilm::ALL {
            let model = PeelModel::for_film(film);
            let s = model.stress_kpa;
            assert!(s.low < s.mid && s.mid < s.high, "{}", film.label());
            assert!(s.mid >= 12.0 && s.mid <= 19.0);
        }
    }

    #[test]
    fn a_tip_holds_its_section_times_its_strength() {
        let model = PeelModel::default();
        let held = model.tip_capacity_n(0.15);
        let section = std::f64::consts::PI * 0.15 * 0.15;
        assert!((held.mid - 8.0 * section).abs() < 1e-12);
        assert!(held.low < held.mid && held.mid < held.high);
    }
}
