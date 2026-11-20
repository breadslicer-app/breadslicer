/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! How a printed body shares a pull among its tips (#241).
//!
//! What is printed of a body is far stiffer than the thin tips that hold it,
//! so it moves as one rigid piece, and every tip is an equal spring along its
//! axis. A pull `F` at point `e` of the plate then splits as on a bolt group:
//!
//! ```text
//! P_i = F/n + dᵢ · I⁻¹ · F·(e − c)
//! ```
//!
//! with `c` the tips' centroid, `dᵢ` tip `i`'s offset from it and
//! `I = Σ dᵢ dᵢᵀ` their second moment. It balances the pull and its moment.
//!
//! A share `λ` of the pull comes sideways, at the film, a height `h` above
//! the tips: its moment `λ·F·h` tilts the body. Its direction is not known,
//! so each tip takes it from the direction worst for it, `λ·F·h·|I⁻¹ dᵢ|`.
//!
//! Tips in a line cannot take a moment across it, nor one tip any moment:
//! the body would turn on them. The share leaves out what they cannot take
//! (a pseudo-inverse of `I`): there the tips bend, which a share along their
//! axes does not see.
//!
//! On a forest where each tip has its own trunk this matches a beam frame of
//! the supports closely; where tips hang from another tip's trunk, the frame
//! finds the worst tip about a third more loaded.

/// The sums a group of tips is known by, which merge when two bodies join.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TipSums {
    count: u32,
    sx: f64,
    sy: f64,
    sxx: f64,
    sxy: f64,
    syy: f64,
    sz: f64,
}

impl TipSums {
    /// Adds a tip that touches the body at `xy_mm`, `z_mm` above the plate.
    pub fn add(&mut self, xy_mm: [f64; 2], z_mm: f64) {
        let [x, y] = xy_mm;
        self.count += 1;
        self.sx += x;
        self.sy += y;
        self.sxx += x * x;
        self.sxy += x * y;
        self.syy += y * y;
        self.sz += z_mm;
    }

    /// Adds every tip of `other`.
    pub fn merge(&mut self, other: &Self) {
        self.count += other.count;
        self.sx += other.sx;
        self.sy += other.sy;
        self.sxx += other.sxx;
        self.sxy += other.sxy;
        self.syy += other.syy;
        self.sz += other.sz;
    }

    /// The number of tips.
    #[inline]
    pub const fn count(&self) -> u32 {
        self.count
    }

    /// The group, `None` without a tip.
    pub fn group(&self) -> Option<TipGroup> {
        if self.count == 0 {
            return None;
        }
        let n = f64::from(self.count);
        let centroid = [self.sx / n, self.sy / n];
        let a = (self.sxx - self.sx * centroid[0]).max(0.0);
        let b = self.sxy - self.sx * centroid[1];
        let c = (self.syy - self.sy * centroid[1]).max(0.0);
        Some(TipGroup {
            count: self.count,
            centroid,
            mean_z_mm: self.sz / n,
            inverse: pseudo_inverse(a, b, c),
        })
    }
}

/// Moments under this share of the largest are what tips in a line give:
/// a spread 100 times narrower one way than the other.
const RANK_TOLERANCE: f64 = 1e-4;

/// The pseudo-inverse of the symmetric `[[a, b], [b, c]]`.
fn pseudo_inverse(a: f64, b: f64, c: f64) -> [[f64; 2]; 2] {
    let half_trace = 0.5 * (a + c);
    let radius = (0.5 * (a - c)).hypot(b);
    let (large, small) = (half_trace + radius, half_trace - radius);
    if !(large > 1e-12) {
        return [[0.0; 2]; 2];
    }
    if small > RANK_TOLERANCE * large {
        let det = a * c - b * b;
        return [[c / det, -b / det], [-b / det, a / det]];
    }
    // One direction only: u uᵀ / λ, u its eigenvector.
    let (ux, uy) = if (large - a).abs() + b.abs() > (large - c).abs() + b.abs() {
        (b, large - a)
    } else {
        (large - c, b)
    };
    let norm = ux.hypot(uy);
    if norm <= 0.0 {
        // Diagonal: the larger axis.
        return if a >= c {
            [[1.0 / a, 0.0], [0.0, 0.0]]
        } else {
            [[0.0, 0.0], [0.0, 1.0 / c]]
        };
    }
    let (ux, uy) = (ux / norm, uy / norm);
    [
        [ux * ux / large, ux * uy / large],
        [ux * uy / large, uy * uy / large],
    ]
}

/// The tips that hold one body, ready to share a pull.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TipGroup {
    count: u32,
    centroid: [f64; 2],
    mean_z_mm: f64,
    inverse: [[f64; 2]; 2],
}

/// One pull on a body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TipLoadCase {
    /// The pull, in newtons.
    pub force_n: f64,
    /// Where it pulls: the centroid of the force on the plate, mm.
    pub at_mm: [f64; 2],
    /// The height of the film it is peeled from, mm above the plate.
    pub z_mm: f64,
    /// The share of it that comes sideways.
    pub lateral_share: f64,
}

impl TipGroup {
    /// The number of tips.
    #[inline]
    pub const fn count(&self) -> u32 {
        self.count
    }

    /// Their centroid on the plate.
    #[inline]
    pub const fn centroid(&self) -> [f64; 2] {
        self.centroid
    }

    /// The load on the tip at `tip_mm` of this group, in newtons: positive
    /// pulls on it. The tips' loads add up to the pull.
    pub fn load_n(&self, tip_mm: [f64; 2], case: &TipLoadCase) -> f64 {
        let n = f64::from(self.count);
        let d = [tip_mm[0] - self.centroid[0], tip_mm[1] - self.centroid[1]];
        let k = &self.inverse;
        let kd = [
            k[0][0] * d[0] + k[0][1] * d[1],
            k[1][0] * d[0] + k[1][1] * d[1],
        ];
        let e = [
            case.at_mm[0] - self.centroid[0],
            case.at_mm[1] - self.centroid[1],
        ];
        let lever = case.force_n * (kd[0] * e[0] + kd[1] * e[1]);
        let height = (case.z_mm - self.mean_z_mm).max(0.0);
        let lateral =
            case.lateral_share.max(0.0) * case.force_n.abs() * height * kd[0].hypot(kd[1]);
        case.force_n / n + lever + lateral
    }
}

#[cfg(test)]
#[path = "tests_share.rs"]
mod tests_share;
